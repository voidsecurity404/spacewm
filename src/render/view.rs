//! Everything about the 3D view of one output: camera, temporary controls, the scene
//! renderer and frame statistics.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::{
    backend::renderer::{
        element::texture::TextureRenderElement, gles::GlesTexture, glow::GlowRenderer,
    },
    utils::{Physical, Size},
};

use glam::{Vec2, Vec4};
use smithay::reexports::glow;

use super::{
    camera::Camera,
    panel_texture::{window_is_opaque, PanelTextureCache},
    scene::{PanelDraw, SceneRenderer},
};
use crate::{input::camera_controls::CameraControls, world::World};

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
        *self = Self {
            enabled,
            ..Self::default()
        };
    }
}

pub struct View3D {
    pub camera: Camera,
    pub controls: CameraControls,
    pub scene: Option<SceneRenderer>,
    pub panel_textures: Option<PanelTextureCache>,
    /// The scene must be re-rendered (camera moved, resized, shaders reloaded, ...).
    pub dirty: bool,
    pub shader_dir: Option<PathBuf>,
    pub stats: FrameStats,
    pending_shader_reload: bool,
    /// Set if creating the scene renderer failed; we fall back to a blank frame instead
    /// of retrying (and logging) every frame.
    failed: bool,
    last_update: Option<Instant>,
    last_size: Size<i32, Physical>,
}

pub struct Prepared {
    pub element: Option<TextureRenderElement<GlesTexture>>,
    pub scene_rendered: bool,
    /// Something is animating (keys held): render again next frame.
    pub continuous: bool,
}

impl View3D {
    pub fn new(shader_dir: Option<PathBuf>) -> Self {
        Self {
            camera: Camera::default(),
            controls: CameraControls::default(),
            scene: None,
            panel_textures: None,
            dirty: true,
            shader_dir,
            stats: FrameStats::default(),
            pending_shader_reload: false,
            failed: false,
            last_update: None,
            last_size: Size::default(),
        }
    }

    pub fn request_shader_reload(&mut self) {
        self.pending_shader_reload = true;
        self.dirty = true;
    }

    /// Run before binding the output framebuffer: advance the camera and re-render the
    /// scene texture if anything changed.
    pub fn prepare(
        &mut self,
        renderer: &mut GlowRenderer,
        size: Size<i32, Physical>,
        world: &mut World,
    ) -> Prepared {
        // Time since the previous update, for frame-rate independent movement. After an
        // idle period the gap is huge, so assume one 60 Hz frame instead of teleporting.
        let now = Instant::now();
        let dt = match self.last_update {
            Some(last) if self.controls.is_moving() => {
                now.duration_since(last).as_secs_f32().min(0.1)
            }
            _ => 1.0 / 60.0,
        };
        self.last_update = Some(now);
        if self.controls.update(&mut self.camera, dt) {
            self.dirty = true;
        }
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
            return Prepared {
                element: None,
                scene_rendered: false,
                continuous,
            };
        };
        if std::mem::take(&mut self.pending_shader_reload) {
            scene.reload_shaders(renderer);
        }

        // New window content → new panel textures → the scene must be redrawn.
        let textures = self
            .panel_textures
            .get_or_insert_with(|| PanelTextureCache::new(renderer));
        textures.updates = 0;
        if textures.update(renderer, world) {
            self.dirty = true;
        }
        self.stats.panel_updates += textures.updates;

        let mut scene_rendered = false;
        if self.dirty {
            let draws = panel_draws(world, textures, &self.camera);
            match scene.render(renderer, &self.camera, size, &draws) {
                Ok(()) => scene_rendered = true,
                Err(err) => tracing::error!("scene render failed: {err}"),
            }
            self.dirty = false;
        }
        Prepared {
            element: scene.element(),
            scene_rendered,
            continuous,
        }
    }
}

/// What to draw for every visible panel that has a texture, in draw order.
fn panel_draws(world: &World, textures: &PanelTextureCache, camera: &Camera) -> Vec<PanelDraw> {
    world
        .iter_sorted_for_render(camera.position, |p| window_is_opaque(&p.window))
        .into_iter()
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
            let geo_rect = Vec4::new(
                x0,
                y0,
                x0 + p.size_px.0 as f32 * sx,
                y0 + p.size_px.1 as f32 * sy,
            );
            Some(PanelDraw {
                texture,
                model: p.bbox_model_matrix(),
                tex_size,
                geo_rect,
                opacity: p.opacity,
                border_px: 0.0,
                border_color: Vec4::ZERO,
                opaque: window_is_opaque(&p.window) && p.opacity >= 1.0,
            })
        })
        .collect()
}
