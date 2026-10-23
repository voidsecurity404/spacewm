//! winit backend: spacewm as a window inside the current desktop.
//!
//! The host compositor (Hyprland, GNOME, ...) gives us a window with an EGL surface; we
//! render our whole desktop into it, and the host forwards keyboard/mouse events. It is
//! the safe way to develop — a hang only freezes one window.

use std::time::Duration;

use smithay::{
    backend::{
        egl::EGLDevice,
        renderer::{damage::OutputDamageTracker, glow::GlowRenderer, ImportDma},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    output::Output,
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            LoopHandle, RegistrationToken,
        },
        wayland_server::DisplayHandle,
        winit::{dpi::LogicalSize, window::WindowAttributes},
    },
    utils::Transform,
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
};

use super::Backend;
use crate::{
    error::SpaceError,
    render::{self, screenshot},
    state::SpaceState,
    wayland::output::{create_output, resize_output},
};

/// We don't know the host monitor's refresh rate; 60 Hz is the safe assumption.
const REFRESH_MHZ: i32 = 60_000;

pub struct WinitBackend {
    graphics: WinitGraphicsBackend<GlowRenderer>,
    pub output: Output,
    damage_tracker: OutputDamageTracker,
    pub dmabuf_state: DmabufState,
    _dmabuf_global: DmabufGlobal,
    /// Number of upcoming frames that must be redrawn in full (after a resize the buffer
    /// contents are undefined, and EGL keeps 2-3 buffers in rotation).
    full_redraw: u8,
    loop_handle: LoopHandle<'static, SpaceState>,
    /// The renderer was used outside a frame (dmabuf import, screenshot readback), which
    /// leaves the EGL context current WITHOUT our window surface. Querying the buffer age
    /// then fails (EGL_BAD_SURFACE), so the next frame just repaints fully instead.
    surface_not_current: bool,
    /// Pending "estimated vblank" timer for frame callbacks when a redraw had no damage.
    frame_callback_timer: Option<RegistrationToken>,
}

impl WinitBackend {
    pub fn init(
        dh: &DisplayHandle,
        loop_handle: &LoopHandle<'static, SpaceState>,
    ) -> Result<Self, SpaceError> {
        let attributes = WindowAttributes::default()
            .with_title("spacewm (nested)")
            .with_surface_size(LogicalSize::new(1280.0, 800.0))
            .with_visible(true);
        let (mut graphics, event_loop) = winit::init_from_attributes::<GlowRenderer>(attributes)
            .map_err(|e| SpaceError::Backend(format!("winit init failed: {e}")))?;

        let size = graphics.window_size();
        // Flipped180: an EGL window surface has its origin at the bottom-left, while
        // smithay renders with the origin at the top-left. The output transform tells the
        // renderer to flip the final image so it appears upright in the host window.
        let output = create_output(
            dh,
            "spacewm-1",
            "nested",
            size,
            REFRESH_MHZ,
            Transform::Flipped180,
        );
        let damage_tracker = OutputDamageTracker::from_output(&output);

        log_gl_info(graphics.renderer());

        let (dmabuf_state, dmabuf_global) = create_dmabuf_global(dh, graphics.renderer());

        loop_handle
            .insert_source(event_loop, |event, _, state| handle_event(state, event))
            .map_err(|e| {
                SpaceError::Backend(format!("failed to insert winit source: {}", e.error))
            })?;

        // The host window has its own cursor; we draw the client-requested cursor ourselves
        // (it may be an I-beam, a hand, ...), so hide the host one to avoid two cursors.
        graphics.window().set_cursor_visible(false);
        graphics.window().request_redraw();

        Ok(Self {
            graphics,
            output,
            damage_tracker,
            dmabuf_state,
            _dmabuf_global: dmabuf_global,
            full_redraw: 4,
            loop_handle: loop_handle.clone(),
            frame_callback_timer: None,
            surface_not_current: true,
        })
    }

    /// Renderer access from outside the render path (see `surface_not_current`).
    pub fn renderer(&mut self) -> &mut GlowRenderer {
        self.surface_not_current = true;
        self.graphics.renderer()
    }

    pub fn schedule_redraw(&mut self) {
        // The host delivers `Redraw` when it is ready for a new frame (its frame callback),
        // which paces us to the host's refresh rate.
        self.graphics.window().request_redraw();
    }
}

/// Advertise linux-dmabuf with the formats our GPU can import. Version 4 "feedback" also
/// tells clients which GPU (render node) to allocate on, which Mesa needs.
pub fn create_dmabuf_global(
    dh: &DisplayHandle,
    renderer: &mut GlowRenderer,
) -> (DmabufState, DmabufGlobal) {
    let formats = renderer.dmabuf_formats();
    let render_node = EGLDevice::device_for_display(renderer.egl_context().display())
        .ok()
        .and_then(|device| device.try_get_render_node().ok().flatten());
    let mut state = DmabufState::new();
    let global = match render_node.and_then(|node| {
        DmabufFeedbackBuilder::new(node.dev_id(), formats.clone())
            .build()
            .ok()
    }) {
        Some(feedback) => {
            tracing::debug!("global: zwp_linux_dmabuf_v1 (v5 with feedback)");
            state.create_global_with_default_feedback::<SpaceState>(dh, &feedback)
        }
        None => {
            tracing::warn!("no render node for dmabuf feedback; using dmabuf v3");
            state.create_global::<SpaceState>(dh, formats)
        }
    };
    (state, global)
}

pub fn log_gl_info(renderer: &mut GlowRenderer) {
    use smithay::reexports::glow::HasContext;
    let info = renderer.with_context(|gl| unsafe {
        (
            gl.get_parameter_string(smithay::reexports::glow::VERSION),
            gl.get_parameter_string(smithay::reexports::glow::RENDERER),
        )
    });
    if let Ok((version, gpu)) = info {
        tracing::info!(%version, %gpu, "GL context ready");
    }
}

fn handle_event(state: &mut SpaceState, event: WinitEvent) {
    match event {
        WinitEvent::Resized { size, .. } => {
            if let Backend::Winit(b) = &mut state.backend {
                resize_output(&b.output, size, REFRESH_MHZ);
                b.full_redraw = 4;
                tracing::debug!(w = size.w, h = size.h, "nested window resized");
            }
            state.queue_redraw();
        }
        WinitEvent::Input(event) => crate::input::process_input_event(state, event),
        WinitEvent::Redraw => render(state),
        WinitEvent::CloseRequested => state.quit(),
        WinitEvent::Focus(false) => {
            // The host took keyboard focus away while keys may be held: we will never get
            // their releases, so release them now to avoid stuck keys in clients.
            crate::input::release_all_keys(state);
        }
        WinitEvent::Focus(true) => {}
    }
}

fn render(state: &mut SpaceState) {
    let Backend::Winit(b) = &mut state.backend else {
        return;
    };
    let output = b.output.clone();
    let size = b.graphics.window_size();
    let scale = output.current_scale().fractional_scale().into();
    let time = state.start_time.elapsed();

    // Buffer age = how many frames old the back buffer's contents are; the damage tracker
    // only repaints what changed since then. 0 forces a full repaint.
    let age = if b.full_redraw > 0 || b.surface_not_current || !state.pending_screenshots.is_empty()
    {
        b.surface_not_current = false;
        b.full_redraw = b.full_redraw.saturating_sub(1);
        0
    } else {
        b.graphics.buffer_age().unwrap_or(0)
    };

    let output_origin = state
        .space
        .output_geometry(&output)
        .map(|g| g.loc.to_f64())
        .unwrap_or_default();
    let pointer = state.pointer_location - output_origin;

    let (renderer, mut framebuffer) = match b.graphics.bind() {
        Ok(bound) => bound,
        Err(err) => {
            tracing::error!("failed to bind winit surface: {err}");
            return;
        }
    };
    let overlay = render::pointer_elements(
        renderer,
        &mut state.cursor_status,
        &mut state.cursor_theme,
        state.dnd_icon.as_ref(),
        pointer,
        scale,
        time,
    );
    let damage = match render::render_2d(
        renderer,
        &mut framebuffer,
        age,
        &output,
        &state.space,
        &overlay,
        &mut b.damage_tracker,
    ) {
        Ok(result) => result.damage.cloned(),
        Err(err) => {
            tracing::error!("rendering failed: {err:?}");
            None
        }
    };

    let capture = (!state.pending_screenshots.is_empty()).then(|| {
        screenshot::start_capture(renderer, &framebuffer, size, output.current_transform())
    });
    drop(framebuffer);

    if let Some(damage) = &damage {
        if let Err(err) = b.graphics.submit(Some(damage)) {
            tracing::warn!("failed to submit frame: {err}");
        }
    }
    if let Some(capture) = capture {
        let captured = capture.and_then(|c| screenshot::finish_capture(b.graphics.renderer(), c));
        crate::ipc::finish_screenshots(
            &mut state.pending_screenshots,
            captured,
            &state.screenshot_tx,
        );
        b.surface_not_current = true;
    }
    if damage.is_some() {
        render::send_frame_callbacks(
            &output,
            &state.space,
            &state.cursor_status,
            state.dnd_icon.as_ref(),
            time,
        );
    } else {
        // Nothing changed, so nothing was presented and the host won't pace us. Send frame
        // callbacks after one frame interval instead of immediately, or a client that keeps
        // requesting callbacks without drawing would spin us at 100% CPU.
        if b.frame_callback_timer.is_none() {
            let token = b
                .loop_handle
                .insert_source(
                    Timer::from_duration(Duration::from_micros(16_667)),
                    |_, _, state| {
                        if let Backend::Winit(b) = &mut state.backend {
                            b.frame_callback_timer = None;
                            let output = b.output.clone();
                            let time = state.start_time.elapsed();
                            render::send_frame_callbacks(
                                &output,
                                &state.space,
                                &state.cursor_status,
                                state.dnd_icon.as_ref(),
                                time,
                            );
                        }
                        TimeoutAction::Drop
                    },
                )
                .ok();
            b.frame_callback_timer = token;
        }
    }
}
