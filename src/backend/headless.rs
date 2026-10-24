//! Headless backend: a compositor without a screen.
//!
//! Renders every frame into an offscreen GPU texture on the render node
//! (/dev/dri/renderD*), which needs no DRM master and no session, so it can run next to
//! a normal desktop. Frames are paced by a 60 Hz timer as if there were a monitor.
//! Combined with IPC input injection and screenshots, this lets tests drive real clients
//! (foot, GTK apps) and look at the result.

use std::{
    fs::OpenOptions,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::{gbm::GbmDevice, Fourcc},
        drm::DrmDeviceFd,
        egl::{EGLContext, EGLDisplay},
        renderer::{
            damage::OutputDamageTracker, gles::GlesTexture, glow::GlowRenderer, Bind, Offscreen,
        },
    },
    output::Output,
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            LoopHandle, RegistrationToken,
        },
        wayland_server::DisplayHandle,
    },
    utils::{DeviceFd, Physical, Size, Transform},
    wayland::dmabuf::{DmabufGlobal, DmabufState},
};

use super::Backend;
use crate::cli::RenderMode;
use crate::{
    cli::Cli,
    error::SpaceError,
    render::{self, screenshot},
    state::SpaceState,
    wayland::output::create_output,
};

const REFRESH_MHZ: i32 = 60_000;
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

pub struct HeadlessBackend {
    renderer: GlowRenderer,
    /// The "screen": an offscreen texture we render into.
    target: GlesTexture,
    size: Size<i32, Physical>,
    pub output: Output,
    damage_tracker: OutputDamageTracker,
    pub dmabuf_state: DmabufState,
    _dmabuf_global: DmabufGlobal,
    loop_handle: LoopHandle<'static, SpaceState>,
    redraw_timer: Option<RegistrationToken>,
    last_frame: Option<Instant>,
    /// The texture keeps its contents between frames, so after the first frame the
    /// "buffer age" is always 1.
    rendered_once: bool,
}

impl HeadlessBackend {
    pub fn init(
        cli: &Cli,
        dh: &DisplayHandle,
        loop_handle: &LoopHandle<'static, SpaceState>,
    ) -> Result<Self, SpaceError> {
        let node = match &cli.headless_device {
            Some(path) => path.clone(),
            None => find_render_node()?,
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&node)
            .map_err(|e| SpaceError::Backend(format!("cannot open {}: {e}", node.display())))?;
        let fd = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(file)));
        let gbm = GbmDevice::new(fd)
            .map_err(|e| SpaceError::Backend(format!("gbm on {}: {e}", node.display())))?;
        // SAFETY: the GBM device outlives the display (the display keeps it alive).
        let display = unsafe { EGLDisplay::new(gbm) }
            .map_err(|e| SpaceError::Backend(format!("EGL display: {e}")))?;
        let context = EGLContext::new(&display)
            .map_err(|e| SpaceError::Backend(format!("EGL context: {e}")))?;
        // SAFETY: the context is fresh and only used by this renderer.
        let mut renderer = unsafe { GlowRenderer::new(context) }
            .map_err(|e| SpaceError::Render(format!("renderer: {e}")))?;
        tracing::info!(device = %node.display(), "headless backend on render node");
        super::winit::log_gl_info(&mut renderer);

        let size: Size<i32, Physical> = cli.headless_size.into();
        let target: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())
            .map_err(|e| SpaceError::Render(format!("offscreen target: {e}")))?;

        let output =
            create_output(dh, "headless-1", "headless", size, REFRESH_MHZ, Transform::Normal);
        let damage_tracker = OutputDamageTracker::from_output(&output);
        let (dmabuf_state, dmabuf_global) = super::winit::create_dmabuf_global(dh, &mut renderer);

        Ok(Self {
            renderer,
            target,
            size,
            output,
            damage_tracker,
            dmabuf_state,
            _dmabuf_global: dmabuf_global,
            loop_handle: loop_handle.clone(),
            redraw_timer: None,
            last_frame: None,
            rendered_once: false,
        })
    }

    pub fn renderer(&mut self) -> &mut GlowRenderer {
        &mut self.renderer
    }

    /// Render at the next 60 Hz "vblank" after the previous frame.
    pub fn schedule_redraw(&mut self) {
        if self.redraw_timer.is_some() {
            return;
        }
        let now = Instant::now();
        let delay = self
            .last_frame
            .map(|last| (last + FRAME_INTERVAL).saturating_duration_since(now))
            .unwrap_or(Duration::ZERO);
        self.redraw_timer = self
            .loop_handle
            .insert_source(Timer::from_duration(delay), |_, _, state| {
                render(state);
                TimeoutAction::Drop
            })
            .ok();
    }
}

/// The first render node, e.g. /dev/dri/renderD128.
fn find_render_node() -> Result<PathBuf, SpaceError> {
    let mut nodes: Vec<PathBuf> = std::fs::read_dir(Path::new("/dev/dri"))
        .map_err(|e| SpaceError::Backend(format!("/dev/dri: {e}")))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes
        .into_iter()
        .next()
        .ok_or_else(|| SpaceError::Backend("no GPU render node in /dev/dri".into()))
}

fn render(state: &mut SpaceState) {
    let Backend::Headless(b) = &mut state.backend else {
        return;
    };
    b.redraw_timer = None;
    b.last_frame = Some(Instant::now());

    let output = b.output.clone();
    let size = b.size;
    let scale = output.current_scale().fractional_scale().into();
    let time = state.start_time.elapsed();
    let capturing = !state.pending_screenshots.is_empty() || state.screencopy.wants(&output);
    let age = if b.rendered_once && !capturing { 1 } else { 0 };

    let frame_start = Instant::now();
    crate::input::before_frame(state);
    let Backend::Headless(b) = &mut state.backend else {
        return;
    };
    // 3D: advance the camera and re-render the scene texture first. The scene pass uses
    // our own framebuffers, so it must run before the output framebuffer is bound.
    let locked = state.lock.is_locked();
    let mut prepared = (state.render_mode == RenderMode::Scene3D && !locked)
        .then(|| state.view.prepare(&mut b.renderer, size, crate::scene_inputs!(state, time)));

    let mut framebuffer = match b.renderer.bind(&mut b.target) {
        Ok(fb) => fb,
        Err(err) => {
            tracing::error!("failed to bind headless target: {err}");
            return;
        }
    };
    let composite = tracing::trace_span!("composite").entered();
    let result = if locked {
        // Locked: the locker's surface and nothing else (see wayland/session_lock.rs).
        let (elements, clear) = render::lock_elements(&mut b.renderer, &state.lock, &output, scale);
        render::render_with_clear(
            &mut b.renderer,
            &mut framebuffer,
            age,
            &elements,
            &mut b.damage_tracker,
            clear,
        )
    } else {
        let overlay = render::overlay_elements(
            &mut b.renderer,
            crate::overlay_inputs!(state, &output, scale, time),
        );
        match prepared.as_mut() {
            Some(prepared) => {
                let mut elements = overlay;
                if let Some(scene) = prepared.element.take() {
                    elements.push(render::SpaceRenderElement::Texture(scene));
                }
                render::render_3d(
                    &mut b.renderer,
                    &mut framebuffer,
                    age,
                    &elements,
                    &mut b.damage_tracker,
                )
            }
            None => render::render_2d(
                &mut b.renderer,
                &mut framebuffer,
                age,
                &output,
                &state.space,
                &overlay,
                &mut b.damage_tracker,
            ),
        }
    };
    drop(composite);
    let presented = match &result {
        Ok(r) => r.damage.is_some(),
        Err(err) => {
            tracing::error!("rendering failed: {err:?}");
            false
        }
    };
    b.rendered_once = true;
    if presented {
        // No real display: the frame counts as presented now.
        let refresh = output
            .current_mode()
            .map(|m| std::time::Duration::from_secs_f64(1000.0 / m.refresh.max(1) as f64));
        let mut feedback = render::take_presentation_feedback(&output, &state.space, &state.lock);
        feedback.presented(
            render::monotonic_now(),
            refresh.map_or(smithay::wayland::presentation::Refresh::Unknown, |r| {
                smithay::wayland::presentation::Refresh::fixed(r)
            }),
            0,
            smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::empty(),
        );
    }

    let capture = capturing.then(|| {
        screenshot::start_capture(&mut b.renderer, &framebuffer, b.size, output.current_transform())
    });
    drop(framebuffer);
    if let Some(capture) = capture {
        let captured = capture.and_then(|c| screenshot::finish_capture(&mut b.renderer, c));
        crate::wayland::screencopy::deliver(
            &mut state.screencopy,
            &output,
            captured.as_ref().map_err(|e| e.as_str()),
        );
        if !state.pending_screenshots.is_empty() {
            crate::ipc::finish_screenshots(
                &mut state.pending_screenshots,
                captured,
                &state.screenshot_tx,
            );
        }
    }

    render::send_frame_callbacks(
        &output,
        &state.space,
        &state.cursor_status,
        state.dnd_icon.as_ref(),
        &state.lock,
        &render::hidden_panels(state.render_mode, &state.world, &state.view.visible_panels),
        time,
    );

    finish_frame(state, frame_start, prepared);
}

/// Bookkeeping shared by all backends after a frame: stats, and keep rendering while
/// something animates.
pub fn finish_frame(
    state: &mut SpaceState,
    frame_start: Instant,
    prepared: Option<render::view::Prepared>,
) {
    let scene_rendered = prepared.as_ref().is_some_and(|p| p.scene_rendered);
    let (panel_updates, chunk_uploads) =
        prepared.as_ref().map_or((0, 0), |p| (p.panel_updates, p.chunk_uploads));
    state.perf.record_frame(frame_start, frame_start.elapsed(), panel_updates, chunk_uploads);
    state.last_frame = Some(frame_start);
    // A lock waiting for confirmation was just drawn: tell the locker it is in charge.
    state.confirm_lock();
    if state.view.stats.enabled {
        state.view.stats.record(frame_start.elapsed(), scene_rendered);
    }
    if prepared.is_some_and(|p| p.continuous) || state.animating() {
        state.queue_redraw();
    }
}
