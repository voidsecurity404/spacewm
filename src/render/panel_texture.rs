//! Getting window pixels onto the GPU as one texture per panel.
//!
//! How client pixels reach a texture:
//! 1. The client attaches a buffer and commits. `on_commit_buffer_handler` (compositor.rs)
//!    records it; the first time smithay renders the surface it imports the buffer into a
//!    `GlesTexture` — for shm by uploading the pixels (glTexImage2D), for dmabuf by
//!    wrapping the GPU memory as an EGLImage (no copy at all). The texture is cached in
//!    the surface's `RendererSurfaceState`.
//! 2. A window is a TREE of surfaces (toplevel + subsurfaces + popups), each with its own
//!    texture, offset, crop/scale (viewporter) and buffer transform. Sampling only the root
//!    texture would miss video subsurfaces, menus and such.
//! 3. So per panel we keep an offscreen texture and let smithay's normal 2D renderer draw
//!    the whole tree into it (`window.render_elements` + a damage tracker). That handles
//!    every surface feature for us, and the damage tracker repaints only the changed parts.
//! 4. Our 3D shader then samples that one texture on the panel quad, with mipmaps so text
//!    stays smooth instead of shimmering when the panel is far away.
//!
//! Shortcut for the common case (the window you are working in): a panel close enough
//! that its texture is shown 1:1 or magnified, made of one surface (no popups, no
//! subsurfaces, no X11 menus), with a plain buffer (scale 1, not rotated or cropped). Then
//! step 3 would only copy the client's texture pixel for pixel, and mipmaps would never be
//! sampled. So the 3D shader samples the client's own texture instead: no copy, no extra
//! GPU memory. For a dmabuf client that is truly zero-copy, as in HackMatrix; unlike there,
//! anything the shortcut can't show correctly (a menu opens, the panel moves away) falls
//! back to the full path on the next frame.

use std::{borrow::Borrow, collections::HashMap};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
            gles::{GlesRenderer, GlesTexture},
            glow::GlowRenderer,
            utils::{import_surface_tree, with_renderer_surface_state},
            Bind, Renderer, Texture,
        },
    },
    desktop::{PopupManager, Window},
    reexports::glow::{self, HasContext},
    utils::{Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::seat::WaylandFocus,
};

use crate::world::{panel::PanelId, World};

/// GL_EXT_texture_filter_anisotropic enums (not in core GLES 3.0).
const TEXTURE_MAX_ANISOTROPY_EXT: u32 = 0x84FE;
const MAX_TEXTURE_MAX_ANISOTROPY_EXT: u32 = 0x84FF;

/// Panels farther than this get a half-resolution texture: at that distance a 1:1
/// texture would be minified 4x anyway, and it costs a quarter of the GPU work and memory.
pub const LOD_DISTANCE: f32 = 10.0;
/// Panel textures re-rendered per frame at most; the rest wait for the next frames, so a
/// burst (all terminals redrawing at once) can't cause one long frame.
pub const UPDATES_PER_FRAME: usize = 8;
/// A panel counts as "near" (shown 1:1 or magnified, so mipmaps are never sampled) up to
/// this many times its pixel-perfect distance. A little shrinking is fine with plain
/// bilinear filtering.
const NEAR_SLACK: f32 = 1.2;
/// ...and only when seen roughly head-on (cosine of the angle to the panel's normal):
/// at a steep angle the far side of a panel is shrunk a lot even when its middle is close.
const NEAR_FACING: f32 = 0.85;

pub struct PanelTarget {
    pub texture: GlesTexture,
    /// Resolution factor this texture was rendered at (1 or 0.5, see LOD_DISTANCE).
    lod: f64,
    /// Texture size in pixels (the window bbox, possibly scaled down to fit GPU limits).
    pub size: Size<i32, Physical>,
    damage_tracker: OutputDamageTracker,
    rendered_once: bool,
    /// Mip levels allocated.
    levels: i32,
    /// The image changed while the panel was near, and the smaller copies were not
    /// rebuilt (nothing samples them up close). Until they are, sampling is limited to
    /// the full-size level, so a stale small copy can never show.
    mips_stale: bool,
}

/// A panel shown straight from the client's own texture (see the module docs).
struct Direct {
    /// A reference to the client's texture: keeps it alive while we draw it.
    texture: GlesTexture,
    size: Size<i32, Physical>,
    /// The buffer's alpha channel is padding (XRGB): draw it as fully opaque.
    opaque_rgb: bool,
}

/// What the 3D shader samples for one panel.
#[derive(Clone, Copy, Debug)]
pub struct PanelTex {
    pub tex_id: u32,
    pub size: Size<i32, Physical>,
    /// Stored bottom row first (some dmabufs); our own textures are top row first.
    pub flip_y: bool,
    pub opaque_rgb: bool,
    /// The client's own texture: no mipmaps, so it must be sampled with plain bilinear.
    pub direct: bool,
}

pub struct PanelTextureCache {
    targets: HashMap<PanelId, PanelTarget>,
    direct: HashMap<PanelId, Direct>,
    max_size: i32,
    anisotropy: f32,
    pub mipmaps: bool,
    /// The no-copy shortcut for near single-surface panels (`render.direct_panels`).
    pub direct_enabled: bool,
    /// Panel textures re-rendered since the counter was last reset (debug stats).
    pub updates: u32,
}

fn mip_levels(w: i32, h: i32) -> i32 {
    (32 - (w.max(h).max(1) as u32).leading_zeros()) as i32
}

/// Whether the window's main surface declares itself fully opaque over its geometry.
pub fn window_is_opaque(window: &Window) -> bool {
    let Some(surface) = window.wl_surface() else {
        return false;
    };
    let geo = window.geometry();
    with_renderer_surface_state(&surface, |state| {
        state.opaque_regions().is_some_and(|regions| regions.iter().any(|r| r.contains_rect(geo)))
    })
    .unwrap_or(false)
}

impl PanelTextureCache {
    pub fn new(renderer: &mut GlowRenderer) -> Self {
        let (max_size, anisotropy) = renderer
            .with_context(|gl| unsafe {
                let max = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE);
                let aniso =
                    if gl.supported_extensions().contains("GL_EXT_texture_filter_anisotropic") {
                        gl.get_parameter_f32(MAX_TEXTURE_MAX_ANISOTROPY_EXT).min(8.0)
                    } else {
                        1.0
                    };
                (max, aniso)
            })
            .unwrap_or((4096, 1.0));
        tracing::debug!(max_size, anisotropy, "panel textures");
        Self {
            targets: HashMap::new(),
            direct: HashMap::new(),
            max_size,
            anisotropy,
            mipmaps: true,
            direct_enabled: true,
            updates: 0,
        }
    }

    /// Switch mipmapped textures on/off; textures are recreated on the next update.
    pub fn set_mipmaps(&mut self, on: bool) {
        self.mipmaps = on;
        self.targets.clear();
    }

    /// GPU memory of all panel textures (mip chains add about a third). Panels shown
    /// from the client's own texture cost nothing extra.
    pub fn gpu_bytes(&self) -> usize {
        let mip = if self.mipmaps { 4.0 / 3.0 } else { 1.0 };
        self.targets.values().map(|t| ((t.size.w * t.size.h * 4) as f64 * mip) as usize).sum()
    }

    pub fn get(&self, id: PanelId) -> Option<PanelTex> {
        if let Some(d) = self.direct.get(&id) {
            return Some(PanelTex {
                tex_id: d.texture.tex_id(),
                size: d.size,
                flip_y: d.texture.is_y_inverted(),
                opaque_rgb: d.opaque_rgb,
                direct: true,
            });
        }
        self.targets.get(&id).map(|t| PanelTex {
            tex_id: t.texture.tex_id(),
            size: t.size,
            flip_y: false,
            opaque_rgb: false,
            direct: false,
        })
    }

    /// Panels currently shown from the client's own texture (debug stats and tests).
    pub fn direct_count(&self) -> usize {
        self.direct.len()
    }

    /// Re-render the textures of visible panels whose client committed new content (or
    /// whose distance crossed the LOD threshold). Panels outside the view keep their
    /// `content_dirty` and are updated once they come into view.
    ///
    /// `px_per_tan` is the output height over 2·tan(fov/2): divided by a panel's pixels
    /// per metre it gives the distance at which the panel is shown pixel-perfect.
    ///
    /// Returns (a texture changed, so the scene must be redrawn; updates are still
    /// waiting because of the per-frame budget).
    pub fn update(
        &mut self,
        renderer: &mut GlowRenderer,
        world: &mut World,
        visible: &std::collections::HashSet<PanelId>,
        camera_pos: glam::Vec3,
        px_per_tan: f32,
    ) -> (bool, bool) {
        // Drop textures of panels that no longer exist.
        self.targets.retain(|id, _| world.panels().iter().any(|p| p.id == *id));
        self.direct.retain(|id, _| world.panels().iter().any(|p| p.id == *id));

        let mut changed = false;
        // Sizes follow the client even for panels out of view (they aren't retextured, but
        // their place in the world and `spacectl list` must stay right).
        for panel in world.panels_mut() {
            let geo = panel.window.geometry();
            if geo.size.w > 0 && geo.size.h > 0 && (geo.size.w, geo.size.h) != panel.size_px {
                // The client resized (e.g. font size change): the panel grows around its
                // center, because `position` is the center.
                panel.set_size((geo.size.w, geo.size.h));
                changed = true;
            }
        }
        let mut budget = UPDATES_PER_FRAME;
        let mut waiting = false;
        for panel in world.panels_mut().iter_mut().filter(|p| visible.contains(&p.id)) {
            let dist = panel.position.distance(camera_pos);
            let lod = if dist > LOD_DISTANCE { 0.5 } else { 1.0 };
            let facing = panel.normal().dot((camera_pos - panel.position).normalize_or_zero());
            let near = dist <= px_per_tan / panel.pixels_per_unit.max(1.0) * NEAR_SLACK
                && facing.abs() >= NEAR_FACING;
            let simple = self.direct_enabled && near && is_single_surface(panel);
            let was_direct = self.direct.contains_key(&panel.id);

            // The shortcut: show the client's texture itself. Not counted against the
            // budget: it is no extra work, and a dmabuf panel must switch to its new
            // buffer in the same frame (the client may already be drawing into the old
            // one again).
            if simple && (panel.content_dirty || !was_direct) {
                let geo = panel.window.geometry();
                let bbox = panel.window.bbox();
                match direct_texture(renderer, &panel.window, bbox) {
                    Some(direct) => {
                        panel.content_dirty = false;
                        panel.bbox_px = Rectangle::new(bbox.loc - geo.loc, bbox.size);
                        // The copy is not needed meanwhile: free its GPU memory.
                        self.targets.remove(&panel.id);
                        self.direct.insert(panel.id, direct);
                        self.updates += 1;
                        changed = true;
                        continue;
                    }
                    // Unusual buffer (rotated, cropped, YUV video...): the full path.
                    None => {
                        if self.direct.remove(&panel.id).is_some() {
                            changed = true;
                        }
                    }
                }
            }
            if simple && was_direct && self.direct.contains_key(&panel.id) {
                continue; // still shown directly, nothing new
            }
            if self.direct.remove(&panel.id).is_some() {
                changed = true; // back to our own copy, rendered below
            }

            let target = self.targets.get(&panel.id);
            let lod_changed = target.is_some_and(|t| t.lod != lod);
            let missing = target.is_none();
            // Moved away from a panel whose mipmaps were skipped: build them now.
            if !(panel.content_dirty || lod_changed || missing) {
                if !near && target.is_some_and(|t| t.mips_stale) {
                    if let Some(t) = self.targets.get_mut(&panel.id) {
                        match build_mipmaps(renderer, t) {
                            Ok(()) => changed = true,
                            Err(err) => tracing::warn!(panel = panel.id, "mipmaps: {err}"),
                        }
                    }
                }
                continue;
            }
            if budget == 0 {
                waiting = true;
                continue;
            }
            budget -= 1;
            panel.content_dirty = false;
            let geo = panel.window.geometry();
            // X11 menus hang off the window like popups: the texture grows to hold them.
            let bbox = panel
                .x11_children
                .iter()
                .fold(panel.window.bbox_with_popups(), |b, c| b.merge(c.rect()));
            if bbox.size.w <= 0 || bbox.size.h <= 0 {
                continue; // no buffer attached yet
            }
            // Bbox relative to the geometry's top-left corner.
            let bbox_rel = Rectangle::new(bbox.loc - geo.loc, bbox.size);
            if bbox_rel != panel.bbox_px {
                panel.bbox_px = bbox_rel;
                changed = true;
            }
            match self.render_panel(
                renderer,
                panel.id,
                &panel.window,
                &panel.x11_children,
                bbox,
                lod,
                near,
            ) {
                Ok(true) => {
                    self.updates += 1;
                    changed = true;
                }
                Ok(false) => {}
                Err(err) => tracing::warn!(panel = panel.id, "panel texture update failed: {err}"),
            }
        }
        (changed, waiting)
    }

    /// Returns Ok(true) if the texture content changed.
    #[allow(clippy::too_many_arguments)]
    fn render_panel(
        &mut self,
        renderer: &mut GlowRenderer,
        id: PanelId,
        window: &Window,
        children: &[crate::xwayland::X11Child],
        bbox: Rectangle<i32, smithay::utils::Logical>,
        lod: f64,
        near: bool,
    ) -> Result<bool, String> {
        // Very large windows are scaled down to the GPU's texture size limit; far panels
        // to half resolution.
        let fit = (self.max_size as f64 / bbox.size.w.max(bbox.size.h) as f64).min(1.0) * lod;
        let scale = Scale::from(fit);
        let size: Size<i32, Physical> = bbox.size.to_f64().to_physical(scale).to_i32_round();

        if self.targets.get(&id).is_none_or(|t| t.size != size) {
            let mut target = self.create_target(renderer, size)?;
            target.lod = lod;
            self.targets.insert(id, target);
        }
        let Some(target) = self.targets.get_mut(&id) else {
            return Ok(false);
        };

        // Draw the window so its bbox origin lands at texture (0, 0).
        let origin: Point<i32, smithay::utils::Logical> = Point::from((-bbox.loc.x, -bbox.loc.y));
        let location: Point<i32, Physical> = origin.to_f64().to_physical(scale).to_i32_round();
        // Front-most first: X11 menus (newest on top), then the window with its popups.
        let elements_span = tracing::trace_span!("panel_elements").entered();
        let mut elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> = Vec::new();
        for child in children.iter().rev() {
            let at = (origin + child.offset).to_f64().to_physical(scale).to_i32_round();
            elements.extend(AsRenderElements::<GlowRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlowRenderer>,
            >(&child.surface, renderer, at, scale, 1.0));
        }
        elements.extend(AsRenderElements::<GlowRenderer>::render_elements::<
            WaylandSurfaceRenderElement<GlowRenderer>,
        >(window, renderer, location, scale, 1.0));
        drop(elements_span);
        // The texture keeps its previous contents, so after the first frame its "age" is 1.
        let age = if target.rendered_once { 1 } else { 0 };
        let render_span = tracing::trace_span!("panel_render").entered();
        let damaged = {
            let mut fb = renderer.bind(&mut target.texture).map_err(|e| e.to_string())?;
            let result = target
                .damage_tracker
                .render_output(renderer, &mut fb, age, &elements, [0.0, 0.0, 0.0, 0.0])
                .map_err(|e| format!("{e:?}"))?;
            result.damage.is_some()
        };
        target.rendered_once = true;
        drop(render_span);

        let _mip_span = tracing::trace_span!("panel_mipmap").entered();
        if !self.mipmaps {
            return Ok(damaged);
        }
        if near {
            // Up close the smaller copies are never sampled: skip rebuilding them (a full
            // pass over the image on the GPU) until the panel moves away.
            if damaged && !target.mips_stale {
                set_max_level(renderer, &target.texture, 0)?;
                target.mips_stale = true;
            }
            Ok(damaged)
        } else if damaged || target.mips_stale {
            // Also when this update changed nothing but the copies are behind: no other
            // frame may come to fix them.
            build_mipmaps(renderer, target)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn create_target(
        &self,
        renderer: &mut GlowRenderer,
        size: Size<i32, Physical>,
    ) -> Result<PanelTarget, String> {
        let (w, h) = (size.w, size.h);
        let levels = if self.mipmaps { mip_levels(w, h) } else { 1 };
        let anisotropy = self.anisotropy;
        let tex = renderer
            .with_context(|gl| unsafe {
                let tex = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                // Immutable storage with the full mip chain allocated up front.
                gl.tex_storage_2d(glow::TEXTURE_2D, levels, glow::RGBA8, w, h);
                let min = if levels > 1 { glow::LINEAR_MIPMAP_LINEAR } else { glow::LINEAR };
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                // Anisotropic filtering keeps text readable on panels seen at a steep angle.
                if anisotropy > 1.0 {
                    gl.tex_parameter_f32(glow::TEXTURE_2D, TEXTURE_MAX_ANISOTROPY_EXT, anisotropy);
                }
                gl.bind_texture(glow::TEXTURE_2D, None);
                Ok::<_, String>(tex)
            })
            .map_err(|e| e.to_string())??;
        let gles: &GlesRenderer = (*renderer).borrow();
        // SAFETY: a valid RGBA8 texture of this context; smithay deletes it on drop.
        let texture = unsafe {
            GlesTexture::from_raw(gles, Some(glow::RGBA8), false, tex.0.get(), (w, h).into())
        };
        Ok(PanelTarget {
            texture,
            lod: 1.0,
            size,
            damage_tracker: OutputDamageTracker::new(size, 1.0, Transform::Normal),
            rendered_once: false,
            levels,
            mips_stale: false,
        })
    }
}

/// Rebuild the smaller copies (1/2, 1/4, ...) from the full-size image, and let the
/// sampler use them again.
fn build_mipmaps(renderer: &mut GlowRenderer, target: &mut PanelTarget) -> Result<(), String> {
    let tex = glow::NativeTexture(non_zero(target.texture.tex_id())?);
    let top = target.levels - 1;
    renderer
        .with_context(|gl| unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            // GL only fills the levels up to MAX_LEVEL, and while the panel was near that
            // was 0: raise it first, or the copies stay empty and the panel turns invisible.
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, top);
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.bind_texture(glow::TEXTURE_2D, None);
        })
        .map_err(|e| e.to_string())?;
    target.mips_stale = false;
    Ok(())
}

/// Limit sampling to mip levels 0..=`level` (GL_TEXTURE_MAX_LEVEL).
fn set_max_level(
    renderer: &mut GlowRenderer,
    texture: &GlesTexture,
    level: i32,
) -> Result<(), String> {
    let tex = glow::NativeTexture(non_zero(texture.tex_id())?);
    renderer
        .with_context(|gl| unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, level);
            gl.bind_texture(glow::TEXTURE_2D, None);
        })
        .map_err(|e| e.to_string())
}

/// One surface, nothing hanging off it: no subsurfaces (video layers, Firefox's
/// separate parts), no popups (menus, tooltips), no X11 menus.
fn is_single_surface(panel: &crate::world::panel::Panel<Window>) -> bool {
    if !panel.x11_children.is_empty() {
        return false;
    }
    let Some(surface) = panel.window.wl_surface() else {
        return false;
    };
    PopupManager::popups_for_surface(&surface).next().is_none() && !has_mapped_child(&surface)
}

/// Whether a subsurface below `surface` shows anything. Apps often keep unmapped ones
/// around (foot: its decorations, search box, URL hints), which don't count.
fn has_mapped_child(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> bool {
    smithay::wayland::compositor::get_children(surface).iter().any(|child| {
        with_renderer_surface_state(child, |s| s.buffer().is_some()).unwrap_or(false)
            || has_mapped_child(child)
    })
}

/// The client's texture of a single-surface window, if it can be shown as is: same size
/// as the window's bbox (buffer scale 1, not rotated, not cropped or scaled by
/// viewporter) and a plain RGB(A) format (not YUV video, which needs a converting
/// sampler). Imports the newest buffer first, which for shm means uploading what changed.
fn direct_texture(
    renderer: &mut GlowRenderer,
    window: &Window,
    bbox: Rectangle<i32, smithay::utils::Logical>,
) -> Option<Direct> {
    let surface = window.wl_surface()?;
    if let Err(err) = import_surface_tree(renderer, &surface) {
        tracing::debug!("direct panel texture: import failed: {err}");
        return None;
    }
    let context = renderer.context_id();
    with_renderer_surface_state(&surface, |state| {
        if state.buffer_scale() != 1 || state.buffer_transform() != Transform::Normal {
            return None;
        }
        let size = state.buffer_size()?;
        let view = state.view()?;
        let uncropped = view.src.loc.x == 0.0
            && view.src.loc.y == 0.0
            && view.src.size == size.to_f64()
            && view.dst == size;
        if !uncropped || size != bbox.size {
            tracing::trace!(?size, ?bbox, ?view, "direct: size");
            return None;
        }
        let texture = state.texture::<GlesTexture>(context)?.clone();
        let opaque_rgb = match texture.format()? {
            Fourcc::Argb8888 | Fourcc::Abgr8888 => false,
            Fourcc::Xrgb8888 | Fourcc::Xbgr8888 => true,
            other => {
                tracing::trace!(?other, "direct: format");
                return None;
            }
        };
        if (texture.width() as i32, texture.height() as i32) != (size.w, size.h) {
            return None;
        }
        Some(Direct { texture, size: (size.w, size.h).into(), opaque_rgb })
    })
    .flatten()
}

fn non_zero(id: u32) -> Result<std::num::NonZeroU32, String> {
    std::num::NonZeroU32::new(id).ok_or_else(|| "texture id 0".to_string())
}

#[cfg(test)]
mod tests {
    use super::mip_levels;

    #[test]
    fn mip_chain_length() {
        assert_eq!(mip_levels(1, 1), 1);
        assert_eq!(mip_levels(1000, 700), 10); // 1000 → 500 → ... → 1
        assert_eq!(mip_levels(1024, 16), 11);
    }
}
