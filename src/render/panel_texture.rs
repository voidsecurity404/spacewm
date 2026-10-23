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

use std::{borrow::Borrow, collections::HashMap};

use smithay::{
    backend::renderer::{
        damage::OutputDamageTracker,
        element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
        gles::{GlesRenderer, GlesTexture},
        glow::GlowRenderer,
        utils::with_renderer_surface_state,
        Bind,
    },
    desktop::Window,
    reexports::glow::{self, HasContext},
    utils::{Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::seat::WaylandFocus,
};

use crate::world::{panel::PanelId, World};

/// GL_EXT_texture_filter_anisotropic enums (not in core GLES 3.0).
const TEXTURE_MAX_ANISOTROPY_EXT: u32 = 0x84FE;
const MAX_TEXTURE_MAX_ANISOTROPY_EXT: u32 = 0x84FF;

pub struct PanelTarget {
    pub texture: GlesTexture,
    /// Texture size in pixels (the window bbox, possibly scaled down to fit GPU limits).
    pub size: Size<i32, Physical>,
    damage_tracker: OutputDamageTracker,
    rendered_once: bool,
}

pub struct PanelTextureCache {
    targets: HashMap<PanelId, PanelTarget>,
    max_size: i32,
    anisotropy: f32,
    pub mipmaps: bool,
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
        Self { targets: HashMap::new(), max_size, anisotropy, mipmaps: true, updates: 0 }
    }

    pub fn get(&self, id: PanelId) -> Option<&PanelTarget> {
        self.targets.get(&id)
    }

    /// Re-render the textures of panels whose client committed new content. Returns true
    /// if any texture changed (the scene must then be redrawn).
    pub fn update(&mut self, renderer: &mut GlowRenderer, world: &mut World) -> bool {
        // Drop textures of panels that no longer exist.
        self.targets.retain(|id, _| world.panels().iter().any(|p| p.id == *id));

        let mut changed = false;
        for panel in world.panels_mut().iter_mut().filter(|p| p.content_dirty) {
            panel.content_dirty = false;
            let geo = panel.window.geometry();
            let bbox = panel.window.bbox_with_popups();
            if bbox.size.w <= 0 || bbox.size.h <= 0 {
                continue; // no buffer attached yet
            }
            if (geo.size.w, geo.size.h) != panel.size_px {
                // The client resized (e.g. font size change): the panel grows around its
                // center, because `position` is the center.
                panel.set_size((geo.size.w, geo.size.h));
                changed = true;
            }
            // Bbox relative to the geometry's top-left corner.
            let bbox_rel = Rectangle::new(bbox.loc - geo.loc, bbox.size);
            if bbox_rel != panel.bbox_px {
                panel.bbox_px = bbox_rel;
                changed = true;
            }
            match self.render_panel(renderer, panel.id, &panel.window, bbox) {
                Ok(true) => {
                    self.updates += 1;
                    changed = true;
                }
                Ok(false) => {}
                Err(err) => tracing::warn!(panel = panel.id, "panel texture update failed: {err}"),
            }
        }
        changed
    }

    /// Returns Ok(true) if the texture content changed.
    fn render_panel(
        &mut self,
        renderer: &mut GlowRenderer,
        id: PanelId,
        window: &Window,
        bbox: Rectangle<i32, smithay::utils::Logical>,
    ) -> Result<bool, String> {
        // Very large windows are scaled down to the GPU's texture size limit.
        let fit = (self.max_size as f64 / bbox.size.w.max(bbox.size.h) as f64).min(1.0);
        let scale = Scale::from(fit);
        let size: Size<i32, Physical> = bbox.size.to_f64().to_physical(scale).to_i32_round();

        if self.targets.get(&id).is_none_or(|t| t.size != size) {
            let target = self.create_target(renderer, size)?;
            self.targets.insert(id, target);
        }
        let Some(target) = self.targets.get_mut(&id) else {
            return Ok(false);
        };

        // Draw the window so its bbox origin lands at texture (0, 0).
        let origin: Point<i32, smithay::utils::Logical> = Point::from((-bbox.loc.x, -bbox.loc.y));
        let location: Point<i32, Physical> = origin.to_f64().to_physical(scale).to_i32_round();
        let elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> =
            AsRenderElements::<GlowRenderer>::render_elements(
                window, renderer, location, scale, 1.0,
            );
        // The texture keeps its previous contents, so after the first frame its "age" is 1.
        let age = if target.rendered_once { 1 } else { 0 };
        let damaged = {
            let mut fb = renderer.bind(&mut target.texture).map_err(|e| e.to_string())?;
            let result = target
                .damage_tracker
                .render_output(renderer, &mut fb, age, &elements, [0.0, 0.0, 0.0, 0.0])
                .map_err(|e| format!("{e:?}"))?;
            result.damage.is_some()
        };
        target.rendered_once = true;

        if damaged && self.mipmaps {
            let tex = glow::NativeTexture(non_zero(target.texture.tex_id())?);
            renderer
                .with_context(|gl| unsafe {
                    // Rebuild the smaller copies (1/2, 1/4, ...) from the new full-size image.
                    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                    gl.generate_mipmap(glow::TEXTURE_2D);
                    gl.bind_texture(glow::TEXTURE_2D, None);
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(damaged)
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
            size,
            damage_tracker: OutputDamageTracker::new(size, 1.0, Transform::Normal),
            rendered_once: false,
        })
    }
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
