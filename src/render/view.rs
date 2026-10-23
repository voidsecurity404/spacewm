//! Everything about the 3D view of one output: camera, temporary controls, the scene
//! renderer and frame statistics.

use std::{path::PathBuf, time::Duration};

use smithay::{
    backend::renderer::{
        element::texture::TextureRenderElement, gles::GlesTexture, glow::GlowRenderer,
    },
    utils::{Physical, Size},
};

use glam::{IVec3, Mat4, Vec2, Vec3, Vec4};
use smithay::{
    backend::renderer::{damage::OutputDamageTracker, Bind, Offscreen},
    input::pointer::CursorImageStatus,
    reexports::glow,
    utils::{Logical, Point, Transform},
};

use super::{
    camera::Camera,
    cursor::{cursor_elements, CursorThemeCache},
    panel_texture::{window_is_opaque, PanelTextureCache},
    scene::{PanelDraw, SceneRenderer},
    voxels::RENDER_DISTANCE,
};
use crate::{
    input::{camera_controls::CameraControls, mode::Highlight},
    world::{mesh::ChunkMeshData, panel::PanelId, World},
};

#[derive(Default)]
pub struct FrameStats {
    pub enabled: bool,
    frames: u32,
    scene_renders: u32,
    pub panel_updates: u32,
    cpu_time: Duration,
    max_cpu_time: Duration,
}

impl FrameStats {
    pub fn record(&mut self, cpu_time: Duration, scene_rendered: bool) {
        self.frames += 1;
        self.scene_renders += scene_rendered as u32;
        self.cpu_time += cpu_time;
        self.max_cpu_time = self.max_cpu_time.max(cpu_time);
    }

    /// Log and reset (called once per second while the debug overlay is on).
    pub fn report(&mut self, camera: &Camera) {
        let avg_ms = if self.frames > 0 {
            self.cpu_time.as_secs_f64() * 1000.0 / self.frames as f64
        } else {
            0.0
        };
        let p = camera.position;
        tracing::info!(
            fps = self.frames,
            scene_renders = self.scene_renders,
            panel_updates = self.panel_updates,
            avg_frame_ms = format!("{avg_ms:.2}"),
            max_frame_ms = format!("{:.2}", self.max_cpu_time.as_secs_f64() * 1000.0),
            pos = format!("({:.2}, {:.2}, {:.2})", p.x, p.y, p.z),
            yaw_deg = format!("{:.1}", camera.yaw.to_degrees()),
            pitch_deg = format!("{:.1}", camera.pitch.to_degrees()),
            "debug"
        );
        let enabled = self.enabled;
        *self = Self { enabled, ..Self::default() };
    }
}

pub struct View3D {
    pub camera: Camera,
    pub controls: CameraControls,
    pub scene: Option<SceneRenderer>,
    pub panel_textures: Option<PanelTextureCache>,
    /// The cursor drawn on the focused panel in Window mode.
    cursor_target: Option<CursorTarget>,
    /// The scene must be re-rendered (camera moved, resized, shaders reloaded, ...).
    pub dirty: bool,
    pub shader_dir: Option<PathBuf>,
    pub stats: FrameStats,
    pending_shader_reload: bool,
    /// Set if creating the scene renderer failed; we fall back to a blank frame instead
    /// of retrying (and logging) every frame.
    failed: bool,
    last_size: Size<i32, Physical>,
}

pub struct Prepared {
    pub element: Option<TextureRenderElement<GlesTexture>>,
    pub scene_rendered: bool,
    /// Something is animating (keys held): render again next frame.
    pub continuous: bool,
}

/// The client's cursor, drawn on a panel (Window mode).
pub struct PanelCursor<'a> {
    pub panel: PanelId,
    /// Window-geometry px.
    pub at: Point<f64, Logical>,
    pub status: &'a mut CursorImageStatus,
    pub theme: &'a mut CursorThemeCache,
    pub time: Duration,
}

/// What the scene needs from the rest of the compositor this frame.
pub struct SceneInputs<'a> {
    pub world: &'a mut World,
    pub highlight: Highlight,
    pub cursor: Option<PanelCursor<'a>>,
    /// Chunk meshes from the mesher, waiting for upload.
    pub chunk_meshes: &'a mut Vec<ChunkMeshData>,
    /// The block the crosshair is on (gets an outline).
    pub outline: Option<IVec3>,
}

/// Border colors (straight RGBA) and widths in window px.
const HOVER_COLOR: Vec4 = Vec4::new(0.45, 0.75, 1.0, 0.9);
const HOVER_BORDER_PX: f32 = 6.0;
const FOCUS_COLOR: Vec4 = Vec4::new(1.0, 0.62, 0.18, 1.0);
const FOCUS_BORDER_PX: f32 = 6.0;
/// Window mode: the other panels fade a little so the focused one stands out.
const UNFOCUSED_OPACITY: f32 = 0.85;

/// Cursor images are rendered into a square texture with the hotspot at its center.
const CURSOR_TEX: i32 = 128;

struct CursorTarget {
    texture: GlesTexture,
    damage_tracker: OutputDamageTracker,
    rendered_once: bool,
}

impl View3D {
    pub fn new(shader_dir: Option<PathBuf>) -> Self {
        Self {
            camera: Camera::default(),
            controls: CameraControls::default(),
            scene: None,
            panel_textures: None,
            cursor_target: None,
            dirty: true,
            shader_dir,
            stats: FrameStats::default(),
            pending_shader_reload: false,
            failed: false,
            last_size: Size::default(),
        }
    }

    pub fn request_shader_reload(&mut self) {
        self.pending_shader_reload = true;
        self.dirty = true;
    }

    /// Run before binding the output framebuffer: update panel textures and re-render
    /// the scene texture if anything changed.
    pub fn prepare(
        &mut self,
        renderer: &mut GlowRenderer,
        size: Size<i32, Physical>,
        inputs: SceneInputs<'_>,
    ) -> Prepared {
        let SceneInputs { world, highlight, cursor, chunk_meshes, outline } = inputs;
        if size != self.last_size {
            self.camera.set_aspect(size.w, size.h);
            self.last_size = size;
            self.dirty = true;
        }

        if self.scene.is_none() && !self.failed {
            match SceneRenderer::new(renderer, self.shader_dir.clone()) {
                Ok(scene) => self.scene = Some(scene),
                Err(err) => {
                    tracing::error!("3D renderer unavailable: {err}");
                    self.failed = true;
                }
            }
        }
        let continuous = self.controls.is_moving();
        let Some(scene) = self.scene.as_mut() else {
            return Prepared { element: None, scene_rendered: false, continuous };
        };
        if std::mem::take(&mut self.pending_shader_reload) {
            scene.reload_shaders(renderer);
        }

        // New window content → new panel textures → the scene must be redrawn.
        let textures = self.panel_textures.get_or_insert_with(|| PanelTextureCache::new(renderer));
        textures.updates = 0;
        if textures.update(renderer, world) {
            self.dirty = true;
        }
        self.stats.panel_updates += textures.updates;

        // The cursor on the focused panel: re-render its image if it changed.
        let cursor_draw = match cursor {
            Some(c) => match update_cursor_texture(
                &mut self.cursor_target,
                renderer,
                c.status,
                c.theme,
                c.time,
            ) {
                Ok(changed) => {
                    if changed {
                        self.dirty = true;
                    }
                    self.cursor_target
                        .as_ref()
                        .and_then(|t| cursor_draw(world, c.panel, c.at, &t.texture))
                }
                Err(err) => {
                    tracing::warn!("cursor texture: {err}");
                    None
                }
            },
            None => None,
        };

        // Newly meshed chunks (a few per frame).
        if scene.upload_chunks(renderer, chunk_meshes) > 0 {
            self.dirty = true;
        }

        let mut scene_rendered = false;
        if self.dirty {
            let mut draws = panel_draws(world, textures, &self.camera, highlight);
            draws.extend(cursor_draw);
            let voxel_frame = scene.voxels.visible(&self.camera, RENDER_DISTANCE, outline);
            match scene.render(renderer, &self.camera, size, &draws, &voxel_frame) {
                Ok(()) => scene_rendered = true,
                Err(err) => tracing::error!("scene render failed: {err}"),
            }
            self.dirty = false;
        }
        // More meshes waiting: come back next frame.
        let continuous = continuous || !chunk_meshes.is_empty();
        Prepared { element: scene.element(), scene_rendered, continuous }
    }
}

impl View3D {
    /// The last rendered scene, stretched to an output of `size` (logical px).
    pub fn scene_element_sized(
        &self,
        size: Size<i32, Logical>,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        self.scene.as_ref()?.element_sized(Some(size))
    }
}

/// Render the current cursor image into a small texture, hotspot at the center.
/// Returns true if its content changed.
fn update_cursor_texture(
    target: &mut Option<CursorTarget>,
    renderer: &mut GlowRenderer,
    status: &mut CursorImageStatus,
    theme: &mut CursorThemeCache,
    time: Duration,
) -> Result<bool, String> {
    if target.is_none() {
        let texture: GlesTexture = renderer
            .create_buffer(
                smithay::backend::allocator::Fourcc::Abgr8888,
                (CURSOR_TEX, CURSOR_TEX).into(),
            )
            .map_err(|e| e.to_string())?;
        // GL's default minification filter expects mipmaps this texture doesn't have; a
        // texture sampled that way is "incomplete" and reads as opaque black.
        if let Some(id) = std::num::NonZeroU32::new(texture.tex_id()) {
            let _ = renderer.with_context(|gl| unsafe {
                use smithay::reexports::glow::HasContext;
                gl.bind_texture(glow::TEXTURE_2D, Some(glow::NativeTexture(id)));
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
            });
        }
        *target = Some(CursorTarget {
            texture,
            damage_tracker: OutputDamageTracker::new(
                (CURSOR_TEX, CURSOR_TEX),
                1.0,
                Transform::Normal,
            ),
            rendered_once: false,
        });
    }
    let Some(t) = target.as_mut() else {
        return Ok(false);
    };
    let center = Point::from((CURSOR_TEX as f64 / 2.0, CURSOR_TEX as f64 / 2.0));
    let elements = cursor_elements(renderer, status, theme, center, 1.0.into(), time);
    let age = if t.rendered_once { 1 } else { 0 };
    let mut fb = renderer.bind(&mut t.texture).map_err(|e| e.to_string())?;
    let result = t
        .damage_tracker
        .render_output(renderer, &mut fb, age, &elements, [0.0, 0.0, 0.0, 0.0])
        .map_err(|e| format!("{e:?}"))?;
    t.rendered_once = true;
    Ok(result.damage.is_some())
}

/// The cursor quad: centered on the hotspot, 1 mm in front of the panel so it never
/// z-fights with it, and scaled like the panel content (texture px = window px).
fn cursor_draw(
    world: &World,
    panel: PanelId,
    at: Point<f64, Logical>,
    texture: &GlesTexture,
) -> Option<PanelDraw> {
    let p = world.panels().iter().find(|p| p.id == panel)?;
    let ppu = p.pixels_per_unit;
    let (gw, gh) = (p.size_px.0 as f32, p.size_px.1 as f32);
    let local = Vec3::new((at.x as f32 - gw / 2.0) / ppu, (gh / 2.0 - at.y as f32) / ppu, 0.001);
    let size = CURSOR_TEX as f32 / ppu;
    let model = Mat4::from_rotation_translation(p.rotation, p.position)
        * Mat4::from_translation(local)
        * Mat4::from_scale(Vec3::new(size, size, 1.0));
    let tex = glow::NativeTexture(std::num::NonZeroU32::new(texture.tex_id())?);
    let n = CURSOR_TEX as f32;
    Some(PanelDraw {
        texture: tex,
        model,
        tex_size: Vec2::splat(n),
        geo_rect: Vec4::new(0.0, 0.0, n, n),
        opacity: 1.0,
        border_px: 0.0,
        border_color: Vec4::ZERO,
        opaque: false,
    })
}

/// What to draw for every visible panel that has a texture, in draw order.
fn panel_draws(
    world: &World,
    textures: &PanelTextureCache,
    camera: &Camera,
    highlight: Highlight,
) -> Vec<PanelDraw> {
    let mut draws: Vec<(PanelDraw, f32)> = world
        .panels()
        .iter()
        .filter(|p| p.visible)
        .filter_map(|p| {
            let target = textures.get(p.id)?;
            let texture = glow::NativeTexture(std::num::NonZeroU32::new(target.texture.tex_id())?);
            let tex_size = Vec2::new(target.size.w as f32, target.size.h as f32);
            // Geometry inside the texture: it starts at -bbox.loc (bbox is relative to the
            // geometry), scaled like the texture if the window had to be shrunk.
            let sx = tex_size.x / p.bbox_px.size.w.max(1) as f32;
            let sy = tex_size.y / p.bbox_px.size.h.max(1) as f32;
            let x0 = -p.bbox_px.loc.x as f32 * sx;
            let y0 = -p.bbox_px.loc.y as f32 * sy;
            let geo_rect =
                Vec4::new(x0, y0, x0 + p.size_px.0 as f32 * sx, y0 + p.size_px.1 as f32 * sy);
            let (border_px, border_color) = if highlight.focused == Some(p.id) {
                (FOCUS_BORDER_PX, FOCUS_COLOR)
            } else if highlight.hovered == Some(p.id) {
                (HOVER_BORDER_PX, HOVER_COLOR)
            } else {
                (0.0, Vec4::ZERO)
            };
            let dimmed = highlight.focused.is_some_and(|f| f != p.id);
            let opacity = p.opacity * if dimmed { UNFOCUSED_OPACITY } else { 1.0 };
            let draw = PanelDraw {
                texture,
                model: p.bbox_model_matrix(),
                tex_size,
                geo_rect,
                opacity,
                border_px,
                border_color,
                opaque: window_is_opaque(&p.window) && opacity >= 1.0,
            };
            Some((draw, p.position.distance_squared(camera.position)))
        })
        .collect();
    // Opaque first, near to far (the depth test then skips hidden pixels early); then
    // translucent far to near so each blends over what is behind it. Decided on the
    // FINAL opacity, so panels dimmed in Window mode count as translucent.
    draws.sort_by(|(a, da), (b, db)| match (a.opaque, b.opaque) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (true, true) => da.total_cmp(db),
        (false, false) => db.total_cmp(da),
    });
    draws.into_iter().map(|(d, _)| d).collect()
}
