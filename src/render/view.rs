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

use super::{camera::Camera, scene::SceneRenderer};
use crate::input::camera_controls::CameraControls;

#[derive(Default)]
pub struct FrameStats {
    pub enabled: bool,
    frames: u32,
    scene_renders: u32,
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
    pub fn prepare(&mut self, renderer: &mut GlowRenderer, size: Size<i32, Physical>) -> Prepared {
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
        let mut scene_rendered = false;
        if self.dirty {
            match scene.render(renderer, &self.camera, size) {
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
