//! Nested backend: spacewm as a window inside the current desktop (via winit).
//!
//! The host compositor (Hyprland, GNOME, ...) gives us a window with an EGL surface; we
//! render our whole desktop into it, and the host forwards keyboard/mouse events. It is
//! the safe way to develop — a hang only freezes one window.
//!
//! Why not smithay's ready-made winit backend? First-person mouse look needs RELATIVE
//! motion: you can turn around forever, the cursor never hits the window edge. A Wayland
//! host only provides that through a locked pointer + the relative-pointer protocol,
//! which winit delivers as `DeviceEvent::PointerMotion` — and smithay's backend drops
//! device events. So we drive winit ourselves and reuse smithay's EGL pieces.
//!
//! Pointer lock: in 3D mode the host pointer is locked to our window and hidden while it
//! has focus. Ctrl+Alt+G releases it (like a VM); clicking into the window locks it again.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        egl::{
            context::{GlAttributes, PixelFormatRequirements},
            native::XlibWindow,
            EGLContext, EGLDevice, EGLDisplay, EGLSurface,
        },
        input::{Axis, AxisSource, ButtonState, InputTime, KeyState},
        renderer::{damage::OutputDamageTracker, glow::GlowRenderer, Bind, ImportDma},
    },
    input::{keyboard::Keycode, pointer::AxisFrame},
    output::Output,
    reexports::{
        calloop::{
            self,
            generic::Generic,
            timer::{TimeoutAction, Timer},
            EventSource, Interest, LoopHandle, Poll, PostAction, Readiness, RegistrationToken,
            Token, TokenFactory,
        },
        wayland_server::DisplayHandle,
        winit::{
            application::ApplicationHandler,
            dpi::LogicalSize,
            event::{
                ButtonSource, DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta,
                PointerSource, WindowEvent,
            },
            event_loop::{
                pump_events::{EventLoopExtPumpEvents, PumpStatus},
                ActiveEventLoop, EventLoop,
            },
            platform::scancode::PhysicalKeyExtScancode,
            raw_window_handle::{HasWindowHandle, RawWindowHandle},
            window::{CursorGrabMode, Window as WinitWindow, WindowAttributes, WindowId},
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Size, Transform},
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
};

use super::Backend;
use crate::{
    cli::RenderMode,
    error::SpaceError,
    render::{self, screenshot},
    state::SpaceState,
    wayland::output::{create_output, resize_output},
};

/// We don't know the host monitor's refresh rate; 60 Hz is the safe assumption.
const REFRESH_MHZ: i32 = 60_000;

/// What the host told us, already translated out of winit types.
#[derive(Debug)]
enum NestedEvent {
    Resized(Size<i32, Physical>),
    Redraw,
    CloseRequested,
    Focus(bool),
    /// Linux evdev scancode.
    Key {
        scancode: u32,
        pressed: bool,
    },
    /// Absolute position inside the window, physical px.
    PointerMoved {
        x: f64,
        y: f64,
    },
    /// Raw relative motion (only while the pointer is locked).
    RelativeMotion {
        dx: f64,
        dy: f64,
    },
    Button {
        code: u32,
        pressed: bool,
    },
    Scroll(MouseScrollDelta),
    PointerLeft,
}

/// winit's event-loop callbacks: create the window once, then queue events.
struct App {
    attributes: WindowAttributes,
    window: Option<Arc<dyn WinitWindow>>,
    create_error: Option<String>,
    events: Vec<NestedEvent>,
}

/// Linux evdev button codes (linux/input-event-codes.h).
fn button_code(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 0x110,
        MouseButton::Right => 0x111,
        MouseButton::Middle => 0x112,
        MouseButton::Back => 0x116,
        MouseButton::Forward => 0x115,
        other => 0x110 + other as u32,
    }
}

impl ApplicationHandler for App {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_none() {
            match event_loop.create_window(self.attributes.clone()) {
                Ok(window) => self.window = Some(Arc::from(window)),
                Err(err) => self.create_error = Some(err.to_string()),
            }
        }
    }

    fn window_event(&mut self, _: &dyn ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let ev = match event {
            WindowEvent::SurfaceResized(size) => {
                NestedEvent::Resized((size.width as i32, size.height as i32).into())
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let Some(w) = &self.window else { return };
                let size = w.surface_size();
                NestedEvent::Resized((size.width as i32, size.height as i32).into())
            }
            WindowEvent::RedrawRequested => NestedEvent::Redraw,
            WindowEvent::CloseRequested => NestedEvent::CloseRequested,
            WindowEvent::Focused(focused) => NestedEvent::Focus(focused),
            // Key repeat is done by clients (wl_keyboard repeat info), so drop host repeats.
            WindowEvent::KeyboardInput { event, is_synthetic, .. }
                if !is_synthetic && !event.repeat =>
            {
                let Some(scancode) = event.physical_key.to_scancode() else {
                    return;
                };
                NestedEvent::Key { scancode, pressed: event.state == ElementState::Pressed }
            }
            WindowEvent::PointerMoved { position, source: PointerSource::Mouse, .. } => {
                NestedEvent::PointerMoved { x: position.x, y: position.y }
            }
            WindowEvent::PointerButton { state, button: ButtonSource::Mouse(button), .. } => {
                NestedEvent::Button {
                    code: button_code(button),
                    pressed: state == ElementState::Pressed,
                }
            }
            WindowEvent::MouseWheel { delta, .. } => NestedEvent::Scroll(delta),
            WindowEvent::PointerLeft { .. } => NestedEvent::PointerLeft,
            _ => return,
        };
        self.events.push(ev);
    }

    fn device_event(&mut self, _: &dyn ActiveEventLoop, _: Option<DeviceId>, event: DeviceEvent) {
        if let DeviceEvent::PointerMotion { delta: (dx, dy) } = event {
            self.events.push(NestedEvent::RelativeMotion { dx, dy });
        }
    }
}

/// calloop event source around winit's event loop.
struct NestedEventSource {
    event_loop: Generic<EventLoop>,
    app: App,
    /// Used to wake ourselves up when events are already queued before sleeping.
    fake_token: Option<Token>,
}

impl NestedEventSource {
    fn pump(&mut self) -> PumpStatus {
        // SAFETY: we never drop the event loop while the source exists.
        let event_loop = unsafe { self.event_loop.get_mut() };
        event_loop.pump_app_events(Some(Duration::ZERO), &mut self.app)
    }
}

impl EventSource for NestedEventSource {
    type Event = NestedEvent;
    type Metadata = ();
    type Ret = ();
    type Error = std::io::Error;

    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = true;

    /// winit may have events queued internally that didn't make its fd readable (another
    /// thread woke it, a redraw request). Drain before calloop sleeps, and wake right away
    /// if anything turned up — otherwise those events would wait for unrelated activity.
    fn before_sleep(&mut self) -> calloop::Result<Option<(Readiness, Token)>> {
        self.pump();
        Ok(if self.app.events.is_empty() {
            None
        } else {
            self.fake_token.map(|t| (Readiness::EMPTY, t))
        })
    }

    fn process_events<F>(
        &mut self,
        _: Readiness,
        _: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let status = self.pump();
        for event in std::mem::take(&mut self.app.events) {
            callback(event, &mut ());
        }
        Ok(match status {
            PumpStatus::Continue => PostAction::Continue,
            PumpStatus::Exit(_) => PostAction::Remove,
        })
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.fake_token = Some(factory.token());
        self.event_loop.register(poll, factory)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.event_loop.reregister(poll, factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.event_loop.unregister(poll)
    }
}

pub struct WinitBackend {
    window: Arc<dyn WinitWindow>,
    renderer: GlowRenderer,
    surface: EGLSurface,
    // Must outlive the surface and renderer.
    _display: EGLDisplay,
    /// The host supports partial presentation (EGL_KHR_swap_buffers_with_damage).
    damage_supported: bool,
    bind_size: Option<Size<i32, Physical>>,
    pub output: Output,
    damage_tracker: OutputDamageTracker,
    pub dmabuf_state: DmabufState,
    _dmabuf_global: DmabufGlobal,
    /// Upcoming frames that must be redrawn in full (after a resize the back buffers'
    /// contents are undefined; EGL keeps 2-3 of them in rotation).
    full_redraw: u8,
    loop_handle: LoopHandle<'static, SpaceState>,
    /// Pending "estimated vblank" timer for frame callbacks after a frame without damage.
    frame_callback_timer: Option<RegistrationToken>,
    focused: bool,
    pointer_locked: bool,
    /// The user released the pointer with Ctrl+Alt+G; don't grab it again until a click.
    pointer_released_by_user: bool,
    /// Last absolute pointer position (window px), for the confined-pointer fallback.
    last_abs: Option<Point<f64, Logical>>,
}

impl WinitBackend {
    pub fn init(
        dh: &DisplayHandle,
        loop_handle: &LoopHandle<'static, SpaceState>,
    ) -> Result<Self, SpaceError> {
        let err = |what: &str, e: &dyn std::fmt::Display| SpaceError::Backend(format!("{what}: {e}"));
        let mut event_loop =
            EventLoop::builder().build().map_err(|e| err("winit event loop", &e))?;
        let mut app = App {
            attributes: WindowAttributes::default()
                .with_title("spacewm (nested)")
                .with_surface_size(LogicalSize::new(1280.0, 800.0))
                .with_visible(true),
            window: None,
            create_error: None,
            events: Vec::new(),
        };
        // The window is created inside winit's callback; pump until it exists.
        while app.window.is_none() {
            event_loop.pump_app_events(None, &mut app);
            if let Some(e) = app.create_error.take() {
                return Err(SpaceError::Backend(format!("winit window: {e}")));
            }
        }
        let window = app.window.clone().ok_or_else(|| SpaceError::Backend("no window".into()))?;
        // Events produced while creating the window are not interesting.
        app.events.clear();

        // SAFETY: the window outlives the display (both live in this struct).
        let display =
            unsafe { EGLDisplay::new(window.clone()) }.map_err(|e| err("EGL display", &e))?;
        let attributes =
            GlAttributes { version: (3, 0), profile: None, debug: false, vsync: false };
        let context =
            EGLContext::new_with_config(&display, attributes, PixelFormatRequirements::_8_bit())
                .map_err(|e| err("EGL context", &e))?;
        let pixel_format = context
            .pixel_format()
            .ok_or_else(|| SpaceError::Backend("no EGL pixel format".into()))?;
        let size = window.surface_size();
        let surface = match window.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Wayland(handle)) => {
                // SAFETY: the wl_surface pointer comes from the live winit window.
                let native = unsafe {
                    wayland_egl::WlEglSurface::new_from_raw(
                        handle.surface.as_ptr() as *mut _,
                        size.width as i32,
                        size.height as i32,
                    )
                }
                .map_err(|e| err("wl_egl_window", &e))?;
                // SAFETY: config and display belong together; the native surface is valid.
                unsafe { EGLSurface::new(&display, pixel_format, context.config_id(), native) }
                    .map_err(|e| err("EGL surface", &e))?
            }
            Ok(RawWindowHandle::Xlib(handle)) => unsafe {
                EGLSurface::new(
                    &display,
                    pixel_format,
                    context.config_id(),
                    XlibWindow(handle.window),
                )
            }
            .map_err(|e| err("EGL surface", &e))?,
            _ => return Err(SpaceError::Backend("nested mode needs a Wayland or X11 host".into())),
        };
        let _ = context.unbind();
        // SAFETY: fresh context, used only by this renderer.
        let mut renderer =
            unsafe { GlowRenderer::new(context) }.map_err(|e| err("renderer", &e))?;
        let damage_supported = display.supports_damage();

        let output_size: Size<i32, Physical> = (size.width as i32, size.height as i32).into();
        // Flipped180: an EGL window surface has its origin at the bottom-left, while smithay
        // renders with the origin at the top-left; the output transform makes the final
        // image upright in the host window.
        let output =
            create_output(dh, "spacewm-1", "nested", output_size, REFRESH_MHZ, Transform::Flipped180);
        let damage_tracker = OutputDamageTracker::from_output(&output);
        log_gl_info(&mut renderer);
        let (dmabuf_state, dmabuf_global) = create_dmabuf_global(dh, &mut renderer);

        let source = NestedEventSource {
            event_loop: Generic::new(event_loop, Interest::READ, calloop::Mode::Level),
            app,
            fake_token: None,
        };
        loop_handle
            .insert_source(source, |event, _, state| handle_event(state, event))
            .map_err(|e| SpaceError::Backend(format!("winit event source: {}", e.error)))?;

        // The host's cursor is hidden: we always draw the cursor ourselves (in 2D the one
        // the client asked for, in 3D a crosshair or a cursor on the focused panel).
        window.set_cursor_visible(false);
        window.request_redraw();

        Ok(Self {
            window,
            renderer,
            surface,
            _display: display,
            damage_supported,
            bind_size: None,
            output,
            damage_tracker,
            dmabuf_state,
            _dmabuf_global: dmabuf_global,
            full_redraw: 4,
            loop_handle: loop_handle.clone(),
            frame_callback_timer: None,
            focused: true,
            pointer_locked: false,
            pointer_released_by_user: false,
            last_abs: None,
        })
    }

    pub fn renderer(&mut self) -> &mut GlowRenderer {
        &mut self.renderer
    }

    pub fn schedule_redraw(&mut self) {
        // The host sends RedrawRequested when it wants a new frame (its frame callback),
        // which paces us to the host's refresh rate.
        self.window.request_redraw();
    }

    fn window_size(&self) -> Size<i32, Physical> {
        let s = self.window.surface_size();
        (s.width as i32, s.height as i32).into()
    }

    /// Lock (and hide) the host pointer so we receive relative motion for mouse look.
    fn lock_pointer(&mut self) {
        if self.pointer_locked {
            return;
        }
        // Locked: the host cursor stays put and only relative motion is reported (Wayland
        // pointer-constraints). Not every host supports it (X11), so fall back to Confined.
        let locked = self.window.set_cursor_grab(CursorGrabMode::Locked).is_ok()
            || self.window.set_cursor_grab(CursorGrabMode::Confined).is_ok();
        self.pointer_locked = locked;
        self.pointer_released_by_user = false;
        tracing::debug!(locked, "nested pointer grab");
    }

    fn unlock_pointer(&mut self) {
        if self.pointer_locked {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
            self.pointer_locked = false;
            tracing::debug!("nested pointer released");
        }
    }

    /// Ctrl+Alt+G: give the pointer back to the host desktop (or take it again).
    pub fn toggle_pointer_release(&mut self) {
        if self.pointer_locked {
            self.unlock_pointer();
            self.pointer_released_by_user = true;
            self.window.set_cursor_visible(true);
        } else {
            self.window.set_cursor_visible(false);
            self.lock_pointer();
        }
    }

    /// Lock or release the pointer when switching between 2D and 3D.
    pub fn on_render_mode(&mut self, three_d: bool) {
        if three_d && self.focused && !self.pointer_released_by_user {
            self.lock_pointer();
        } else if !three_d {
            self.unlock_pointer();
        }
    }

    /// Move the GL context onto our surface. Rendering the 3D scene, importing a dmabuf
    /// or reading back a screenshot leaves the context current WITHOUT a surface, and
    /// querying the buffer age then fails with EGL_BAD_SURFACE.
    fn make_current(&self) {
        // SAFETY: the surface belongs to this context's display.
        if let Err(e) =
            unsafe { self.renderer.egl_context().make_current_with_surface(&self.surface) }
        {
            tracing::warn!("make_current failed: {e}");
        }
    }

    fn buffer_age(&self) -> usize {
        if !self.damage_supported {
            return 0;
        }
        self.make_current();
        self.surface.buffer_age().unwrap_or(0).max(0) as usize
    }

    fn submit(&mut self, damage: &[Rectangle<i32, Physical>]) -> Result<(), String> {
        let bind_size = self.bind_size.unwrap_or_else(|| self.window_size());
        // Damage is top-left based; EGL wants bottom-left based rectangles.
        let mut damage: Vec<_> = damage
            .iter()
            .map(|r| Rectangle::new((r.loc.x, bind_size.h - r.loc.y - r.size.h).into(), r.size))
            .collect();
        // Ask the host for a frame callback (it paces our next RedrawRequested).
        self.window.pre_present_notify();
        let damage = (self.damage_supported && !damage.is_empty()).then_some(&mut damage[..]);
        self.surface.swap_buffers(damage).map_err(|e| e.to_string())
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
    let feedback = render_node
        .and_then(|node| DmabufFeedbackBuilder::new(node.dev_id(), formats.clone()).build().ok());
    let global = match feedback {
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

fn handle_event(state: &mut SpaceState, event: NestedEvent) {
    let time = InputTime::now();
    let three_d = state.render_mode == RenderMode::Scene3D;
    let Backend::Winit(b) = &mut state.backend else {
        return;
    };
    match event {
        NestedEvent::Resized(size) => {
            resize_output(&b.output, size, REFRESH_MHZ);
            b.full_redraw = 4;
            tracing::debug!(w = size.w, h = size.h, "nested window resized");
            state.queue_redraw();
        }
        NestedEvent::Redraw => render(state),
        NestedEvent::CloseRequested => state.quit(),
        NestedEvent::Focus(focused) => {
            b.focused = focused;
            if focused {
                b.on_render_mode(three_d);
            } else {
                // The host took focus while keys may be held: we'll never see their
                // releases, so release them now to avoid stuck keys in clients.
                b.pointer_locked = false;
                b.last_abs = None;
                crate::input::release_all_keys(state);
            }
        }
        NestedEvent::Key { scancode, pressed } => {
            // xkb keycodes are evdev codes + 8 (an X11 legacy).
            let key_state = if pressed { KeyState::Pressed } else { KeyState::Released };
            crate::input::handle_key(state, Keycode::new(scancode + 8), key_state, time);
        }
        NestedEvent::RelativeMotion { dx, dy } => {
            // Only used while locked in 3D; in 2D absolute motion drives the cursor.
            if three_d && b.pointer_locked {
                let d = Point::from((dx, dy));
                crate::input::handle_pointer_motion(state, d, d, time);
            }
        }
        NestedEvent::PointerMoved { x, y } => {
            let pos = Point::from((x, y));
            if !three_d {
                crate::input::handle_pointer_motion_absolute(state, pos, time);
            } else if b.pointer_locked {
                // Confined fallback (no relative motion, e.g. an X11 host): turn by the
                // absolute difference. A locked Wayland pointer sends no absolute motion.
                let delta = b.last_abs.map(|last| pos - last).unwrap_or_default();
                b.last_abs = Some(pos);
                if delta.x != 0.0 || delta.y != 0.0 {
                    crate::input::handle_pointer_motion(state, delta, delta, time);
                }
            }
        }
        NestedEvent::PointerLeft => b.last_abs = None,
        NestedEvent::Button { code, pressed } => {
            if three_d && !b.pointer_locked {
                // The first click into the window grabs the pointer; it is not a real click.
                if pressed && b.focused {
                    b.window.set_cursor_visible(false);
                    b.lock_pointer();
                }
                return;
            }
            let bstate = if pressed { ButtonState::Pressed } else { ButtonState::Released };
            crate::input::handle_pointer_button(state, code, bstate, time);
        }
        NestedEvent::Scroll(delta) => {
            let mut frame = AxisFrame::new(time);
            match delta {
                MouseScrollDelta::LineDelta(x, y) => {
                    frame = frame.source(AxisSource::Wheel);
                    // winit: positive = up/left; Wayland: positive = down/right.
                    for (axis, v) in [(Axis::Horizontal, -x as f64), (Axis::Vertical, -y as f64)] {
                        if v != 0.0 {
                            frame = frame.value(axis, v * 15.0).v120(axis, (v * 120.0) as i32);
                        }
                    }
                }
                MouseScrollDelta::PixelDelta(p) => {
                    frame = frame.source(AxisSource::Continuous);
                    for (axis, v) in [(Axis::Horizontal, -p.x), (Axis::Vertical, -p.y)] {
                        if v != 0.0 {
                            frame = frame.value(axis, v);
                        }
                    }
                }
                _ => return,
            }
            crate::input::handle_pointer_axis(state, frame);
        }
    }
}

fn render(state: &mut SpaceState) {
    let Backend::Winit(b) = &mut state.backend else {
        return;
    };
    let output = b.output.clone();
    let size = b.window_size();
    let scale = output.current_scale().fractional_scale().into();
    let time = state.start_time.elapsed();

    // Buffer age = how many frames old the back buffer's contents are; the damage tracker
    // only repaints what changed since then. 0 forces a full repaint.
    let age = if b.full_redraw > 0 || !state.pending_screenshots.is_empty() {
        b.full_redraw = b.full_redraw.saturating_sub(1);
        0
    } else {
        b.buffer_age()
    };

    let frame_start = Instant::now();
    crate::input::before_frame(state);
    let Backend::Winit(b) = &mut state.backend else {
        return;
    };
    // 3D: advance the camera and re-render the scene texture first. The scene pass uses
    // our own framebuffers, so it must run before the output framebuffer is bound.
    let mut prepared = (state.render_mode == RenderMode::Scene3D)
        .then(|| state.view.prepare(&mut b.renderer, size, crate::scene_inputs!(state, time)));

    if Some(size) != b.bind_size {
        b.surface.resize(size.w, size.h, 0, 0);
        b.bind_size = Some(size);
    }
    let mut framebuffer = match b.renderer.bind(&mut b.surface) {
        Ok(fb) => fb,
        Err(e) => {
            tracing::error!("failed to bind nested surface: {e}");
            return;
        }
    };
    let overlay = render::overlay_elements(
        &mut b.renderer,
        crate::overlay_inputs!(state, &output, scale, time),
    );
    let result = match prepared.as_mut() {
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
    };
    let damage = match result {
        Ok(result) => result.damage.cloned(),
        Err(e) => {
            tracing::error!("rendering failed: {e:?}");
            None
        }
    };
    let capture = (!state.pending_screenshots.is_empty()).then(|| {
        screenshot::start_capture(&mut b.renderer, &framebuffer, size, output.current_transform())
    });
    drop(framebuffer);

    if let Some(damage) = &damage {
        if let Err(e) = b.submit(damage) {
            tracing::warn!("failed to submit frame: {e}");
        }
    }
    if let Some(capture) = capture {
        let captured = capture.and_then(|c| screenshot::finish_capture(&mut b.renderer, c));
        crate::ipc::finish_screenshots(
            &mut state.pending_screenshots,
            captured,
            &state.screenshot_tx,
        );
    }
    if damage.is_some() {
        render::send_frame_callbacks(
            &output,
            &state.space,
            &state.cursor_status,
            state.dnd_icon.as_ref(),
            time,
        );
    } else if b.frame_callback_timer.is_none() {
        // Nothing changed, so nothing was presented and the host won't pace us. Send frame
        // callbacks after one frame interval instead of immediately, or a client that keeps
        // requesting callbacks without drawing would spin us at 100% CPU.
        b.frame_callback_timer = b
            .loop_handle
            .insert_source(Timer::from_duration(Duration::from_micros(16_667)), |_, _, state| {
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
            })
            .ok();
    }
    super::headless::finish_frame(state, frame_start, prepared);
}
