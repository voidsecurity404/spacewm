//! DRM/KMS backend: spacewm as a real session on a TTY.
//!
//! Vocabulary:
//! - DRM master: the one process allowed to change what the monitors show. A session
//!   manager (seatd or systemd-logind, through libseat) hands it to us and takes it back
//!   when you switch to another VT — so spacewm never needs to run as root.
//! - KMS (kernel mode setting): programming the display hardware: which resolution, and
//!   which buffer each monitor scans out.
//! - Connector: a physical port (HDMI-A-1, eDP-1). CRTC: the scan-out engine that reads a
//!   framebuffer and streams it to a connector at the mode's timing. Plane: a layer the
//!   CRTC composites in hardware (primary = the frame, cursor = the mouse pointer).
//! - Page flip: at the next vertical blank (vblank) the CRTC switches to the new buffer
//!   atomically, so a half-drawn frame is never visible (no tearing). The kernel tells us
//!   when that happened (a VBlank event) — that is the moment to start the next frame.
//!
//! Safety (a bug here can leave a black screen):
//! - Ctrl+Alt+Backspace quits immediately, before any other keybind (input/mod.rs).
//! - A watchdog quits if a frame was requested but no vblank arrived for 10 s.
//! - The panic hook drops DRM master on every opened GPU before unwinding.
//! - Everything is logged to ~/.local/state/spacewm/spacewm.log.

use std::{
    collections::HashMap,
    os::fd::AsRawFd,
    path::Path,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
        },
        drm::{
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType,
        },
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{glow::GlowRenderer, ImportDma},
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent},
    },
    input::keyboard::LedState,
    output::{Mode as WlMode, Output, PhysicalProperties, Scale},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            LoopHandle, RegistrationToken,
        },
        drm::{
            control::{connector, crtc, Device as ControlDevice, ModeTypeFlags},
            Device as _,
        },
        input::{self as libinput, DeviceCapability, Libinput},
        rustix::fs::OFlags,
        wayland_server::{backend::GlobalId, DisplayHandle},
    },
    utils::{DeviceFd, Physical, Size, Transform},
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
};
use smithay_drm_extras::{
    display_info,
    drm_scanner::{DrmScanEvent, DrmScanner},
};

use super::Backend;
use crate::{
    cli::{Cli, RenderMode},
    error::SpaceError,
    render::{self, SpaceRenderElement},
    state::SpaceState,
};

/// Scan-out formats, in order of preference.
const FORMATS: &[Fourcc] =
    &[Fourcc::Argb8888, Fourcc::Xrgb8888, Fourcc::Abgr8888, Fourcc::Xbgr8888];
/// A requested frame that never reaches the screen this long means the GPU/driver hung.
const WATCHDOG: Duration = Duration::from_secs(10);
/// Above this DPI an output gets scale 2.
const HIDPI: f64 = 200.0;

type Allocator = GbmAllocator<DrmDeviceFd>;
type Exporter = GbmFramebufferExporter<DrmDeviceFd>;
/// The user data of a queued frame: its presentation feedback, reported at the vblank.
type FrameData = Option<smithay::desktop::utils::OutputPresentationFeedback>;
type SpaceDrmOutput = DrmOutput<Allocator, Exporter, FrameData, DrmDeviceFd>;
type SpaceDrmOutputManager = DrmOutputManager<Allocator, Exporter, FrameData, DrmDeviceFd>;

/// Where one output is in its frame cycle.
#[derive(Debug)]
enum RedrawState {
    /// Nothing to do until something changes.
    Idle,
    /// A render is scheduled (idle callback).
    Queued,
    /// A frame is queued for flip; render again after the vblank if `redraw_needed`.
    WaitingForVBlank { redraw_needed: bool, since: Instant },
    /// The last render had no damage, so nothing was flipped and no vblank will come. A
    /// timer stands in for it (frame callbacks still need pacing).
    WaitingForEstimatedVBlank { token: RegistrationToken, redraw_needed: bool },
}

struct OutputSurface {
    output: Output,
    global: GlobalId,
    drm_output: SpaceDrmOutput,
    redraw: RedrawState,
    frame_duration: Duration,
    /// Debug stats (Mod+F3): flips, and frames that took longer than one refresh.
    flips: u32,
    missed: u32,
    last_flip: Option<Instant>,
}

struct GpuDevice {
    node: DrmNode,
    renderer: GlowRenderer,
    manager: SpaceDrmOutputManager,
    scanner: DrmScanner,
    surfaces: HashMap<crtc::Handle, OutputSurface>,
}

pub struct DrmBackend {
    session: LibSeatSession,
    libinput: Libinput,
    gpu: GpuDevice,
    /// Every connected input device (settings are re-applied on config reload; keyboards
    /// also get the Caps/Num Lock LEDs).
    devices: Vec<libinput::Device>,
    /// `[[output]]` sections: mode and scale per connector.
    outputs_config: Vec<crate::config::OutputConfig>,
    loop_handle: LoopHandle<'static, SpaceState>,
    pub dmabuf_state: DmabufState,
    _dmabuf_global: DmabufGlobal,
    dh: DisplayHandle,
    /// We hold the session (our VT is in front).
    active: bool,
}

/// GPU fds the panic hook must release DRM master on (see logging.rs).
fn register_for_panic(fd: i32) {
    if let Ok(mut fds) = crate::logging::DRM_FDS.lock() {
        fds.push(fd);
    }
}

fn backend_err(what: &str, e: &dyn std::fmt::Display) -> SpaceError {
    SpaceError::Backend(format!("{what}: {e}"))
}

impl DrmBackend {
    pub fn init(
        cli: &Cli,
        config: &crate::config::Config,
        dh: &DisplayHandle,
        loop_handle: &LoopHandle<'static, SpaceState>,
    ) -> Result<Self, SpaceError> {
        // 1. Session: device access without root, VT switching.
        let (session, notifier) = LibSeatSession::new()
            .map_err(|e| backend_err("libseat session (are you on a TTY?)", &e))?;
        let seat = session.seat();
        tracing::info!(
            seat,
            backend = std::env::var("LIBSEAT_BACKEND")
                .unwrap_or_else(|_| "auto (seatd or logind)".into()),
            "libseat session opened"
        );

        // 2. GPU: --gpu, else the boot GPU of this seat, else the first one.
        let path = match &cli.gpu {
            Some(p) => p.clone(),
            None => primary_gpu(&seat)
                .ok()
                .flatten()
                .or_else(|| all_gpus(&seat).ok().and_then(|g| g.into_iter().next()))
                .ok_or_else(|| SpaceError::Backend("no GPU found".into()))?,
        };
        let mut gpu = open_gpu(&session, &path, loop_handle)?;

        // 3. dmabuf: GPU apps (Firefox, mpv, games) hand us GPU buffers directly — much
        // faster than copying pixels through shm.
        let formats = gpu.renderer.dmabuf_formats();
        let render_node =
            gpu.node.node_with_type(NodeType::Render).and_then(|n| n.ok()).unwrap_or(gpu.node);
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global =
            match DmabufFeedbackBuilder::new(render_node.dev_id(), formats.clone()).build() {
                Ok(feedback) => {
                    dmabuf_state.create_global_with_default_feedback::<SpaceState>(dh, &feedback)
                }
                Err(_) => dmabuf_state.create_global::<SpaceState>(dh, formats),
            };

        // 4. Input: libinput opens keyboards/mice/touchpads through the session.
        let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
        if cli.drm_no_input {
            tracing::warn!("--drm-no-input: not opening any input devices");
        } else if libinput.udev_assign_seat(&seat).is_err() {
            return Err(SpaceError::Backend(format!("libinput: cannot assign seat {seat}")));
        }
        loop_handle
            .insert_source(LibinputInputBackend::new(libinput.clone()), |event, _, state| {
                on_input(state, event)
            })
            .map_err(|e| backend_err("libinput source", &e.error))?;
        // The devices found by udev_assign_seat are already queued; see drain_device_events.
        loop_handle.insert_idle(drain_device_events);

        // 5. VT switches.
        loop_handle
            .insert_source(notifier, |event, _, state| on_session(state, event))
            .map_err(|e| backend_err("session source", &e.error))?;

        // 6. Monitor hotplug.
        let udev = UdevBackend::new(&seat).map_err(|e| backend_err("udev", &e))?;
        loop_handle
            .insert_source(udev, |event, _, state| on_udev(state, event))
            .map_err(|e| backend_err("udev source", &e.error))?;

        // 7. Outputs that are connected right now.
        let scan = gpu
            .scanner
            .scan_connectors(gpu.manager.device())
            .map_err(|e| backend_err("connector scan", &e))?;
        let mut x = 0;
        for event in scan {
            if let DrmScanEvent::Connected { connector, crtc: Some(crtc) } = event {
                if let Some(surface) =
                    connector_connected(&mut gpu, dh, &config.outputs, connector, crtc, x)
                {
                    x += surface.output.current_mode().map(|m| m.size.w).unwrap_or(0);
                    gpu.surfaces.insert(crtc, surface);
                }
            }
        }
        if gpu.surfaces.is_empty() {
            tracing::warn!("no connected monitors");
        }

        // 8. Watchdog.
        loop_handle
            .insert_source(Timer::from_duration(Duration::from_secs(1)), |_, _, state| {
                watchdog(state);
                TimeoutAction::ToDuration(Duration::from_secs(1))
            })
            .map_err(|e| backend_err("watchdog", &e.error))?;

        Ok(Self {
            session,
            libinput,
            gpu,
            devices: Vec::new(),
            outputs_config: config.outputs.clone(),
            loop_handle: loop_handle.clone(),
            dmabuf_state,
            _dmabuf_global: dmabuf_global,
            dh: dh.clone(),
            active: true,
        })
    }

    pub fn renderer(&mut self) -> &mut GlowRenderer {
        &mut self.gpu.renderer
    }

    pub fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    pub fn outputs(&self) -> Vec<Output> {
        let mut outputs: Vec<_> = self.gpu.surfaces.values().map(|s| s.output.clone()).collect();
        outputs.sort_by_key(|o| o.current_location().x);
        outputs
    }

    /// Something changed: render every output at its next opportunity.
    pub fn schedule_redraw(&mut self) {
        if !self.active {
            return;
        }
        let crtcs: Vec<crtc::Handle> = self.gpu.surfaces.keys().copied().collect();
        for crtc in crtcs {
            let Some(surface) = self.gpu.surfaces.get_mut(&crtc) else { continue };
            match &mut surface.redraw {
                RedrawState::Idle => {
                    surface.redraw = RedrawState::Queued;
                    self.loop_handle.insert_idle(move |state| render(state, crtc));
                }
                RedrawState::Queued => {}
                RedrawState::WaitingForVBlank { redraw_needed, .. }
                | RedrawState::WaitingForEstimatedVBlank { redraw_needed, .. } => {
                    *redraw_needed = true
                }
            }
        }
    }

    pub fn update_led_state(&mut self, led_state: LedState) {
        for device in
            self.devices.iter_mut().filter(|d| d.has_capability(DeviceCapability::Keyboard))
        {
            device.led_update(led_state.into());
        }
    }

    pub fn apply_input_settings(&mut self, settings: &crate::input::mode::InputSettings) {
        for device in &mut self.devices {
            apply_device_settings(device, settings);
        }
    }

    pub fn change_vt(&mut self, vt: i32) {
        tracing::info!(vt, "switching VT");
        if let Err(e) = self.session.change_vt(vt) {
            tracing::warn!("VT switch failed: {e}");
        }
    }

    /// Per-output flips, missed vblanks and refresh rate since the last call (Mod+F3).
    pub fn take_stats(&mut self) -> Vec<(String, u32, u32, f64)> {
        self.gpu
            .surfaces
            .values_mut()
            .map(|s| {
                let hz = 1.0 / s.frame_duration.as_secs_f64();
                let out = (s.output.name(), s.flips, s.missed, hz);
                s.flips = 0;
                s.missed = 0;
                out
            })
            .collect()
    }
}

fn open_gpu(
    session: &LibSeatSession,
    path: &Path,
    loop_handle: &LoopHandle<'static, SpaceState>,
) -> Result<GpuDevice, SpaceError> {
    let node = DrmNode::from_path(path).map_err(|e| backend_err("drm node", &e))?;
    let mut session = session.clone();
    let fd = session
        .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK)
        .map_err(|e| backend_err(&format!("open {}", path.display()), &e))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    register_for_panic(fd.as_raw_fd());

    // `true`: start from a known state with all outputs off (the previous user of the VT
    // may have left anything configured).
    let (drm, notifier) =
        DrmDevice::new(fd.clone(), true).map_err(|e| backend_err("drm device", &e))?;
    let gbm = GbmDevice::new(fd.clone()).map_err(|e| backend_err("gbm", &e))?;
    // SAFETY: the GBM device lives as long as the display.
    let display =
        unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|e| backend_err("EGL display", &e))?;
    let context = EGLContext::new(&display).map_err(|e| backend_err("EGL context", &e))?;
    // SAFETY: fresh context used only by this renderer.
    let mut renderer =
        unsafe { GlowRenderer::new(context) }.map_err(|e| backend_err("renderer", &e))?;
    let driver =
        drm.get_driver().map(|d| d.name().to_string_lossy().into_owned()).unwrap_or_default();
    tracing::info!(path = %path.display(), driver, "GPU opened");
    super::winit::log_gl_info(&mut renderer);

    let allocator =
        GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let exporter = GbmFramebufferExporter::new(gbm.clone(), node.into());
    let render_formats = renderer.egl_context().dmabuf_render_formats().clone();
    let manager = DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        FORMATS.iter().copied(),
        render_formats,
    );

    loop_handle
        .insert_source(notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => on_vblank(state, crtc, metadata.take()),
            DrmEvent::Error(e) => tracing::error!("DRM error: {e}"),
        })
        .map_err(|e| backend_err("drm event source", &e.error))?;

    Ok(GpuDevice { node, renderer, manager, scanner: DrmScanner::new(), surfaces: HashMap::new() })
}

/// Preferred mode, else the largest at the highest refresh rate.
fn pick_mode(connector: &connector::Info) -> Option<smithay::reexports::drm::control::Mode> {
    let modes = connector.modes();
    modes
        .iter()
        .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .or_else(|| {
            modes.iter().max_by_key(|m| {
                let (w, h) = m.size();
                (w as u32 * h as u32, m.vrefresh())
            })
        })
        .copied()
}

/// The mode the config asks for (size, and the refresh rate nearest to the requested
/// one, else the highest), or the connector's preferred mode.
fn choose_mode(
    connector: &connector::Info,
    name: &str,
    request: Option<crate::config::ModeRequest>,
) -> Option<smithay::reexports::drm::control::Mode> {
    if let Some(req) = request {
        let fitting = connector.modes().iter().filter(|m| m.size() == (req.width, req.height));
        let best = match req.refresh {
            Some(hz) => fitting.min_by(|a, b| {
                let da = (a.vrefresh() as f64 - hz).abs();
                let db = (b.vrefresh() as f64 - hz).abs();
                da.total_cmp(&db)
            }),
            None => fitting.max_by_key(|m| m.vrefresh()),
        };
        match best {
            Some(m) => return Some(*m),
            None => tracing::warn!(
                name,
                wanted = format!("{}x{}", req.width, req.height),
                "configured mode not offered by the monitor, using its preferred one"
            ),
        }
    }
    pick_mode(connector)
}

fn connector_connected(
    gpu: &mut GpuDevice,
    dh: &DisplayHandle,
    outputs_config: &[crate::config::OutputConfig],
    connector: connector::Info,
    crtc: crtc::Handle,
    x: i32,
) -> Option<OutputSurface> {
    let name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());
    let configured = outputs_config.iter().find(|o| o.name == name);
    let Some(mode) = choose_mode(&connector, &name, configured.and_then(|o| o.mode_request()))
    else {
        tracing::warn!(name, "connector has no modes");
        return None;
    };
    let info = display_info::for_connector(gpu.manager.device(), connector.handle());
    let make = info.as_ref().and_then(|i| i.make()).unwrap_or_else(|| "Unknown".into());
    let model = info.as_ref().and_then(|i| i.model()).unwrap_or_else(|| "Unknown".into());
    let serial = info.as_ref().and_then(|i| i.serial()).unwrap_or_else(|| "Unknown".into());
    let (mm_w, mm_h) = connector.size().unwrap_or((0, 0));
    let (w, h) = mode.size();

    // Scale from the physical size: 2 on high-density screens so text stays readable.
    let dpi = if mm_w > 0 { w as f64 / (mm_w as f64 / 25.4) } else { 0.0 };
    let auto_scale = if dpi > HIDPI { 2.0 } else { 1.0 };
    let scale = configured.and_then(|o| o.scale).unwrap_or(auto_scale);
    tracing::info!(
        name,
        make,
        model,
        mode = format!("{w}x{h}@{}", mode.vrefresh()),
        size_mm = format!("{mm_w}x{mm_h}"),
        dpi = format!("{dpi:.0}"),
        scale,
        "output"
    );
    for m in connector.modes() {
        let (mw, mh) = m.size();
        tracing::debug!(name, "  mode {mw}x{mh}@{}", m.vrefresh());
    }

    let output = Output::new(
        name.clone(),
        PhysicalProperties {
            size: (mm_w as i32, mm_h as i32).into(),
            subpixel: connector.subpixel().into(),
            make,
            model,
            serial_number: serial,
        },
    );
    let wl_mode = WlMode::from(mode);
    output.change_current_state(
        Some(wl_mode),
        Some(Transform::Normal),
        // Whole numbers stay integer scales (crisper: no resampling of client buffers).
        Some(if scale.fract() == 0.0 {
            Scale::Integer(scale as i32)
        } else {
            Scale::Fractional(scale)
        }),
        Some((x, 0).into()),
    );
    output.set_preferred(wl_mode);
    let global = output.create_global::<SpaceState>(dh);

    let drm_output =
        match gpu.manager.lock().initialize_output::<_, SpaceRenderElement<GlowRenderer>>(
            crtc,
            mode,
            &[connector.handle()],
            &output,
            None,
            &mut gpu.renderer,
            &DrmOutputRenderElements::default(),
        ) {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(name, "cannot drive output: {e}");
                dh.remove_global::<SpaceState>(global);
                return None;
            }
        };
    let frame_duration = Duration::from_secs_f64(1000.0 / wl_mode.refresh.max(1) as f64);
    Some(OutputSurface {
        output,
        global,
        drm_output,
        redraw: RedrawState::Idle,
        frame_duration,
        flips: 0,
        missed: 0,
        last_flip: None,
    })
}

fn on_udev(state: &mut SpaceState, event: UdevEvent) {
    let Backend::Drm(b) = &state.backend else { return };
    let ours = b.gpu.node.dev_id();
    match event {
        UdevEvent::Changed { device_id } if device_id == ours => rescan_connectors(state),
        UdevEvent::Added { path, .. } => {
            tracing::info!(path = %path.display(), "GPU hotplug ignored (one GPU is used)");
        }
        UdevEvent::Removed { device_id } if device_id == ours => {
            tracing::error!("our GPU was removed; quitting");
            state.quit();
        }
        _ => {}
    }
}

/// A monitor was plugged in or out.
fn rescan_connectors(state: &mut SpaceState) {
    let Backend::Drm(b) = &mut state.backend else { return };
    let scan = match b.gpu.scanner.scan_connectors(b.gpu.manager.device()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("connector rescan failed: {e}");
            return;
        }
    };
    let dh = b.dh.clone();
    for event in scan {
        let Backend::Drm(b) = &mut state.backend else { return };
        match event {
            DrmScanEvent::Connected { connector, crtc: Some(crtc) } => {
                let x = b
                    .gpu
                    .surfaces
                    .values()
                    .filter_map(|s| s.output.current_mode())
                    .map(|m| m.size.w)
                    .sum();
                if let Some(surface) =
                    connector_connected(&mut b.gpu, &dh, &b.outputs_config, connector, crtc, x)
                {
                    let output = surface.output.clone();
                    b.gpu.surfaces.insert(crtc, surface);
                    state.space.map_output(&output, (x, 0));
                }
            }
            DrmScanEvent::Disconnected { crtc: Some(crtc), .. } => {
                if let Some(surface) = b.gpu.surfaces.remove(&crtc) {
                    tracing::info!(name = surface.output.name(), "output disconnected");
                    if let RedrawState::WaitingForEstimatedVBlank { token, .. } = surface.redraw {
                        b.loop_handle.remove(token);
                    }
                    dh.remove_global::<SpaceState>(surface.global);
                    state.space.unmap_output(&surface.output);
                }
            }
            _ => {}
        }
    }
    state.view.dirty = true;
    state.queue_redraw();
}

fn on_input(state: &mut SpaceState, mut event: InputEvent<LibinputInputBackend>) {
    match &mut event {
        InputEvent::DeviceAdded { device } => configure_device(state, device),
        InputEvent::DeviceRemoved { device } => {
            tracing::info!(name = %device.name(), "input device removed");
            if let Backend::Drm(b) = &mut state.backend {
                b.devices.retain(|k| k != device);
            }
        }
        _ => {}
    }
    crate::input::process_input_event(state, event);
}

/// Handle device added/removed events that libinput queued by itself.
///
/// `udev_assign_seat`, `suspend` and `resume` put these events into libinput's internal
/// queue without making its fd readable, and the calloop source only looks at the queue
/// when the fd wakes it. Without this, devices would only be configured (tap-to-click,
/// flat acceleration, LEDs) on the first key press or mouse move, seconds later.
fn drain_device_events(state: &mut SpaceState) {
    let Backend::Drm(b) = &mut state.backend else { return };
    // A clone is another handle to the same libinput context, so `state` stays usable.
    let mut libinput = b.libinput.clone();
    if let Err(e) = libinput.dispatch() {
        tracing::warn!("libinput dispatch failed: {e}");
    }
    for event in &mut libinput {
        match event {
            libinput::Event::Device(libinput::event::DeviceEvent::Added(e)) => {
                let device = libinput::event::EventTrait::device(&e);
                on_input(state, InputEvent::DeviceAdded { device });
            }
            libinput::Event::Device(libinput::event::DeviceEvent::Removed(e)) => {
                let device = libinput::event::EventTrait::device(&e);
                on_input(state, InputEvent::DeviceRemoved { device });
            }
            // Only device events are queued at these moments; anything else is stale.
            other => tracing::debug!("dropping queued libinput event {other:?}"),
        }
    }
}

/// Per-device settings: touchpads tap and scroll naturally, mice get a flat acceleration
/// profile (the same hand movement always turns the camera by the same angle — what you
/// want for aiming).
fn configure_device(state: &mut SpaceState, device: &mut libinput::Device) {
    let settings = state.input_settings;
    let caps: Vec<&str> = [
        (DeviceCapability::Keyboard, "keyboard"),
        (DeviceCapability::Pointer, "pointer"),
        (DeviceCapability::Touch, "touch"),
        (DeviceCapability::Gesture, "gesture"),
    ]
    .iter()
    .filter(|(c, _)| device.has_capability(*c))
    .map(|(_, n)| *n)
    .collect();
    tracing::info!(name = %device.name(), caps = caps.join(","), "input device added");

    apply_device_settings(device, &settings);
    if device.has_capability(DeviceCapability::Keyboard) {
        if let Some(led) = state.seat.get_keyboard().map(|k| k.led_state()) {
            device.led_update(led.into());
        }
    }
    if let Backend::Drm(b) = &mut state.backend {
        b.devices.push(device.clone());
    }
}

fn apply_device_settings(
    device: &mut libinput::Device,
    settings: &crate::input::mode::InputSettings,
) {
    if device.config_tap_finger_count() > 0 {
        let _ = device.config_tap_set_enabled(settings.touchpad_tap);
        let _ = device.config_scroll_set_natural_scroll_enabled(settings.touchpad_natural_scroll);
    } else if device.has_capability(DeviceCapability::Pointer) {
        let profile = if settings.mouse_accel_flat {
            libinput::AccelProfile::Flat
        } else {
            libinput::AccelProfile::Adaptive
        };
        let _ = device.config_accel_set_profile(profile);
    }
}

fn on_session(state: &mut SpaceState, event: SessionEvent) {
    let Backend::Drm(b) = &mut state.backend else { return };
    match event {
        SessionEvent::PauseSession => {
            tracing::info!("session paused (VT switched away)");
            b.active = false;
            b.libinput.suspend();
            drain_device_events(state);
            let Backend::Drm(b) = &mut state.backend else { return };
            b.gpu.manager.pause();
            for surface in b.gpu.surfaces.values_mut() {
                if let RedrawState::WaitingForEstimatedVBlank { token, .. } = surface.redraw {
                    b.loop_handle.remove(token);
                }
                surface.redraw = RedrawState::Idle;
            }
            crate::input::release_all_keys(state);
        }
        SessionEvent::ActivateSession => {
            tracing::info!("session active again");
            if let Err(e) = b.libinput.resume() {
                tracing::error!("libinput resume failed: {e:?}");
            }
            drain_device_events(state);
            let Backend::Drm(b) = &mut state.backend else { return };
            // After another VT used the GPU the hardware state is unknown: reset it (full
            // modeset) and redraw every output from scratch.
            if let Err(e) = b.gpu.manager.lock().activate(true) {
                tracing::error!("DRM activate failed: {e}");
            }
            for surface in b.gpu.surfaces.values_mut() {
                surface.drm_output.reset_buffers();
            }
            b.active = true;
            state.view.dirty = true;
            state.queue_redraw();
        }
    }
}

/// Quit if a requested frame never reached the screen (GPU hang, driver bug). An idle
/// desktop renders nothing for hours, so "no frames" alone is NOT a hang.
fn watchdog(state: &mut SpaceState) {
    let Backend::Drm(b) = &state.backend else { return };
    if !b.active {
        return;
    }
    let hung = b.gpu.surfaces.values().find(|s| {
        matches!(s.redraw, RedrawState::WaitingForVBlank { since, .. } if since.elapsed() > WATCHDOG)
    });
    if let Some(surface) = hung {
        tracing::error!(
            output = surface.output.name(),
            "no vblank 10 s after a page flip; quitting"
        );
        state.quit();
    }
}

fn render(state: &mut SpaceState, crtc: crtc::Handle) {
    let frame_start = Instant::now();
    {
        let Backend::Drm(b) = &mut state.backend else { return };
        if !b.active
            || !matches!(b.gpu.surfaces.get(&crtc).map(|s| &s.redraw), Some(RedrawState::Queued))
        {
            return;
        }
    }
    crate::input::before_frame(state);

    let Backend::Drm(b) = &mut state.backend else { return };
    // The 3D scene is rendered once at the size of the first output; other monitors show
    // the same view (mirror). TODO: one camera per monitor ("left monitor looks left").
    let primary_size = b
        .outputs()
        .first()
        .and_then(|o| o.current_mode())
        .map(|m| m.size)
        .unwrap_or_else(|| Size::from((1920, 1080)));
    let Some(surface) = b.gpu.surfaces.get_mut(&crtc) else { return };
    let output = surface.output.clone();
    let Some(mode) = output.current_mode() else { return };
    let size: Size<i32, Physical> = mode.size;
    let scale = output.current_scale().fractional_scale().into();
    let time = state.start_time.elapsed();
    let renderer = &mut b.gpu.renderer;

    let locked = state.lock.is_locked();
    let mut prepared = (state.render_mode == RenderMode::Scene3D && !locked)
        .then(|| state.view.prepare(renderer, primary_size, crate::scene_inputs!(state, time)));
    let (elements, clear) = if locked {
        // Locked: the locker's surface and nothing else (see wayland/session_lock.rs).
        render::lock_elements(renderer, &state.lock, &output, scale)
    } else {
        let mut elements =
            render::overlay_elements(renderer, crate::overlay_inputs!(state, &output, scale, time));
        match prepared.as_mut() {
            Some(p) => {
                // Stretch the shared scene to this output (same size on the primary one).
                if let Some(scene) = state.view.scene_element_sized(size.to_logical(1)) {
                    elements.push(SpaceRenderElement::Texture(scene));
                }
                p.element = None;
            }
            None => {
                // 2D: bars above the windows, wallpapers below (smithay's space renderer
                // does this for the other backends).
                use smithay::wayland::shell::wlr_layer::Layer;
                let map = smithay::desktop::layer_map_for_output(&output);
                elements.extend(render::layer_elements(
                    renderer,
                    &map,
                    &[Layer::Overlay, Layer::Top],
                    scale,
                ));
                elements.extend(render::space_elements(renderer, &state.space, &output, scale));
                elements.extend(render::layer_elements(
                    renderer,
                    &map,
                    &[Layer::Bottom, Layer::Background],
                    scale,
                ));
            }
        }
        let clear = match state.render_mode {
            RenderMode::Scene3D => {
                let [r, g, bl] = render::scene::SKY_COLOR;
                [r, g, bl, 1.0]
            }
            RenderMode::Flat2D => render::CLEAR_COLOR_2D,
        };
        (elements, clear)
    };

    let rendered =
        match surface.drm_output.render_frame(renderer, &elements, clear, FrameFlags::DEFAULT) {
            Ok(r) => !r.is_empty,
            Err(e) => {
                tracing::warn!("render_frame failed: {e:?}");
                false
            }
        };

    if !state.pending_screenshots.is_empty() || state.screencopy.wants(&output) {
        let captured = render::capture_elements(renderer, &elements, size, scale, clear);
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

    if rendered {
        let feedback = render::take_presentation_feedback(&output, &state.space, &state.lock);
        surface.redraw = match surface.drm_output.queue_frame(Some(feedback)) {
            Ok(()) => RedrawState::WaitingForVBlank { redraw_needed: false, since: Instant::now() },
            Err(e) => {
                tracing::warn!("queue_frame failed: {e:?}");
                RedrawState::Idle
            }
        };
    } else {
        // Nothing changed on this output: no flip, so no vblank event. Pace frame
        // callbacks with a timer of one refresh instead.
        let token = b
            .loop_handle
            .insert_source(Timer::from_duration(surface.frame_duration), move |_, _, state| {
                estimated_vblank(state, crtc);
                TimeoutAction::Drop
            })
            .ok();
        surface.redraw = match token {
            Some(token) => RedrawState::WaitingForEstimatedVBlank { token, redraw_needed: false },
            None => RedrawState::Idle,
        };
    }
    render::send_frame_callbacks(
        &output,
        &state.space,
        &state.cursor_status,
        state.dnd_icon.as_ref(),
        &state.lock,
        time,
    );
    super::headless::finish_frame(state, frame_start, prepared);
}

fn on_vblank(
    state: &mut SpaceState,
    crtc: crtc::Handle,
    metadata: Option<smithay::backend::drm::DrmEventMetadata>,
) {
    let animating = state.animating();
    let Backend::Drm(b) = &mut state.backend else { return };
    let Some(surface) = b.gpu.surfaces.get_mut(&crtc) else { return };
    use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
    match surface.drm_output.frame_submitted() {
        Ok(Some(Some(mut feedback))) => {
            // The kernel's vblank timestamp is the moment the frame went on screen.
            let (time, seq, flags) = match metadata {
                Some(smithay::backend::drm::DrmEventMetadata {
                    time: smithay::backend::drm::DrmEventTime::Monotonic(t),
                    sequence,
                }) => (t.into(), sequence as u64, Kind::Vsync | Kind::HwClock | Kind::HwCompletion),
                _ => (render::monotonic_now(), 0, Kind::Vsync),
            };
            let refresh = smithay::wayland::presentation::Refresh::fixed(surface.frame_duration);
            feedback.presented(time, refresh, seq, flags);
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("frame_submitted failed: {e:?}"),
    }
    let now = Instant::now();
    surface.flips += 1;
    if let Some(last) = surface.last_flip {
        // More than 1.5 refreshes between flips while animating: a missed vblank.
        if animating && now.duration_since(last) > surface.frame_duration.mul_f32(1.5) {
            surface.missed += 1;
        }
    }
    surface.last_flip = Some(now);
    let again = animating
        || matches!(surface.redraw, RedrawState::WaitingForVBlank { redraw_needed: true, .. });
    if again {
        surface.redraw = RedrawState::Queued;
        b.loop_handle.insert_idle(move |state| render(state, crtc));
    } else {
        surface.redraw = RedrawState::Idle;
    }
}

fn estimated_vblank(state: &mut SpaceState, crtc: crtc::Handle) {
    let animating = state.animating();
    let Backend::Drm(b) = &mut state.backend else { return };
    let Some(surface) = b.gpu.surfaces.get_mut(&crtc) else { return };
    let again = animating
        || matches!(
            surface.redraw,
            RedrawState::WaitingForEstimatedVBlank { redraw_needed: true, .. }
        );
    if again {
        surface.redraw = RedrawState::Queued;
        b.loop_handle.insert_idle(move |state| render(state, crtc));
    } else {
        surface.redraw = RedrawState::Idle;
    }
}

/// `spacewm --drm-probe`: list GPUs, connectors and modes WITHOUT taking over the display
/// (no session, no DRM master: reading this information needs neither).
pub fn probe() -> anyhow::Result<()> {
    use std::{fs::File, os::fd::AsFd};
    struct Card(File);
    impl AsFd for Card {
        fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
            self.0.as_fd()
        }
    }
    impl smithay::reexports::drm::Device for Card {}
    impl ControlDevice for Card {}

    let gpus = all_gpus("seat0")?;
    if gpus.is_empty() {
        println!("no GPUs on seat0");
    }
    let primary = primary_gpu("seat0").ok().flatten();
    for path in gpus {
        let card = Card(File::open(&path)?);
        let driver = smithay::reexports::drm::Device::get_driver(&card)
            .map(|d| d.name().to_string_lossy().into_owned())
            .unwrap_or_default();
        let mark = if primary.as_ref() == Some(&path) { " (primary)" } else { "" };
        println!("{}{mark}: driver {driver}", path.display());
        let res = card.resource_handles()?;
        for &handle in res.connectors() {
            let info = card.get_connector(handle, false)?;
            let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
            let (mw, mh) = info.size().unwrap_or((0, 0));
            println!("  {name}: {:?}, {mw}x{mh} mm", info.state());
            if let Some(m) = pick_mode(&info) {
                let (w, h) = m.size();
                println!("    would use {w}x{h}@{} ({} modes)", m.vrefresh(), info.modes().len());
            }
        }
        println!("  crtcs: {}", res.crtcs().len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn hidpi_threshold() {
        // A 14" 2560x1600 laptop panel (~301 mm wide) is HiDPI; a 24" 1080p monitor is not.
        let dpi = |w: f64, mm: f64| w / (mm / 25.4);
        assert!(dpi(2560.0, 301.0) > super::HIDPI);
        assert!(dpi(1920.0, 527.0) < super::HIDPI);
    }
}
