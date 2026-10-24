//! `SpaceState`: the single struct that owns all compositor state.
//!
//! smithay's handler traits (CompositorHandler, XdgShellHandler, SeatHandler, ...) are
//! implemented on this one type, and calloop passes `&mut SpaceState` to every event
//! callback. That is why everything lives in ONE struct: any callback can reach any
//! state without Rc<RefCell<..>> or locks.
//!
//! Borrowing note: the renderer lives inside `backend`. Rendering code takes the fields
//! it needs directly (`state.backend.renderer()` + `&state.space`), which the borrow
//! checker allows because they are different fields. Methods on `&mut self` would borrow
//! everything at once, so rendering is written as free functions.

use std::{
    ffi::OsString,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use smithay::wayland::seat::WaylandFocus;
use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{keyboard::XkbConfig, pointer::CursorImageStatus, Seat, SeatState},
    reexports::{
        calloop::{
            channel::{self, Sender},
            LoopHandle, LoopSignal,
        },
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor::{get_parent, CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::{
            data_device::DataDeviceState, primary_selection::PrimarySelectionState,
            wlr_data_control::DataControlState,
        },
        shell::wlr_layer::WlrLayerShellState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        xwayland_shell::XWaylandShellState,
    },
};

use crate::{
    backend::Backend,
    cli::{Cli, RenderMode},
    focus::PointerFocusTarget,
    input::InputState,
    ipc::IpcServer,
    render::{
        cursor::CursorThemeCache,
        screenshot::{ScreenshotDone, ScreenshotRequest},
        view::View3D,
    },
};

/// What has the keyboard besides windows.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Overlay {
    #[default]
    None,
    /// A layer surface with exclusive keyboard interactivity (fuzzel, wofi...): walking
    /// is suspended and a 2D cursor clicks layer surfaces until it closes.
    Layer(smithay::desktop::LayerSurface),
}

/// A drag-and-drop icon: the surface a client attached to the cursor while dragging.
pub struct DndIcon {
    pub surface: WlSurface,
    pub offset: Point<i32, Logical>,
}

pub struct SpaceState {
    /// Output + rendering + input devices. See `backend/mod.rs`.
    pub backend: Backend,
    pub running: bool,
    pub start_time: Instant,
    pub cli: Cli,

    /// Handle to the Wayland server: used to create globals and send events to clients.
    pub display_handle: DisplayHandle,
    /// Handle to the calloop event loop: used to insert event sources later
    /// (timers, IPC connections, child process watchers).
    pub loop_handle: LoopHandle<'static, SpaceState>,
    pub loop_signal: LoopSignal,
    /// The WAYLAND_DISPLAY value clients use to find us (e.g. "wayland-1").
    pub socket_name: OsString,

    // ---- Wayland protocol state (smithay keeps per-protocol bookkeeping here) ----
    /// wl_compositor + wl_subcompositor: surfaces, buffers, commits, subsurfaces.
    pub compositor_state: CompositorState,
    /// xdg_wm_base: turns surfaces into application windows (toplevels) and popups.
    pub xdg_shell_state: XdgShellState,
    /// wl_shm: CPU shared-memory buffers (the client draws in RAM and shares an fd).
    pub shm_state: ShmState,
    /// wl_output + xdg-output: tells clients about our monitors (size, scale).
    /// Never read: holding it keeps the xdg-output global alive.
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    /// wl_seat bookkeeping; `seat` is our one seat ("seat0") with keyboard + pointer.
    pub seat_state: SeatState<SpaceState>,
    pub seat: Seat<SpaceState>,
    /// wl_data_device: clipboard (Ctrl+C / Ctrl+V) and drag-and-drop.
    pub data_device_state: DataDeviceState,
    /// zwp_primary_selection: the "select text, middle-click to paste" clipboard.
    pub primary_selection_state: PrimarySelectionState,
    /// zwlr_data_control: lets clipboard tools (wl-copy, wl-paste, clipboard managers)
    /// read and set the clipboard without having a focused window.
    pub data_control_state: DataControlState,
    /// xwayland_shell_v1: how Xwayland tells us which wl_surface shows which X11 window.
    /// Only the Xwayland client can see this global.
    pub xwayland_shell_state: XWaylandShellState,
    /// Our X11 display and, while X apps run, the Xwayland server (None: X11 disabled).
    pub xwayland: Option<crate::xwayland::Xwayland>,
    /// zwlr_layer_shell_v1: bars, launchers, notifications (wayland/layer_shell.rs).
    pub layer_shell_state: WlrLayerShellState,
    /// zxdg_decoration_manager_v1: we ask apps not to draw title bars.
    /// Never read: holding it keeps the global alive.
    #[allow(dead_code)]
    pub decoration_state: smithay::wayland::shell::xdg::decoration::XdgDecorationState,
    /// ext-image-capture-source + ext-image-copy-capture: screen capture (screencopy.rs).
    #[allow(dead_code)]
    pub image_capture_source_state: smithay::wayland::image_capture_source::ImageCaptureSourceState,
    pub output_capture_source_state:
        smithay::wayland::image_capture_source::OutputCaptureSourceState,
    pub image_copy_capture_state: smithay::wayland::image_copy_capture::ImageCopyCaptureState,
    pub screencopy: crate::wayland::screencopy::Screencopy,
    /// ext-session-lock (wayland/session_lock.rs).
    pub session_lock_state: smithay::wayland::session_lock::SessionLockManagerState,
    pub lock: crate::wayland::session_lock::LockState,
    /// ext-idle-notify (swayidle) and the surfaces holding idle inhibitors (wayland/idle.rs).
    pub idle_notifier_state: smithay::wayland::idle_notify::IdleNotifierState<SpaceState>,
    #[allow(dead_code)]
    pub idle_inhibit_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState,
    pub idle_inhibitors: Vec<WlSurface>,
    /// wp_viewporter: a client shows a crop of its buffer scaled to any size (video
    /// players, browsers). smithay's renderer applies it; holding the state keeps the global.
    #[allow(dead_code)]
    pub viewporter_state: smithay::wayland::viewporter::ViewporterState,
    /// Globals whose state smithay keeps internally; held so they stay alive.
    #[allow(dead_code)]
    pub fractional_scale_state: smithay::wayland::fractional_scale::FractionalScaleManagerState,
    #[allow(dead_code)]
    pub relative_pointer_state: smithay::wayland::relative_pointer::RelativePointerManagerState,
    #[allow(dead_code)]
    pub pointer_constraints_state: smithay::wayland::pointer_constraints::PointerConstraintsState,
    #[allow(dead_code)]
    pub cursor_shape_state: smithay::wayland::cursor_shape::CursorShapeManagerState,
    /// xdg-activation tokens (wayland/activation.rs).
    pub activation_state: smithay::wayland::xdg_activation::XdgActivationState,
    /// wp_presentation: when exactly a frame reached the screen (video players sync to it).
    #[allow(dead_code)]
    pub presentation_state: smithay::wayland::presentation::PresentationState,
    /// zwlr_screencopy_manager_v1 (wf-recorder). Never read: keeps the global.
    #[allow(dead_code)]
    pub wlr_screencopy_global: smithay::reexports::wayland_server::backend::GlobalId,
    /// Something on top of the view that has the keyboard.
    pub overlay: Overlay,
    /// The built-in launcher, while open (it then gets every key).
    pub launcher: Option<crate::launcher::Launcher>,
    /// Media keys, screenshots, auto-lock, battery (desktop/).
    pub desktop: crate::desktop::DesktopState,
    /// The inventory screen (E while walking), while open.
    pub inventory: Option<crate::inventory::Inventory>,
    pub hotbar: crate::inventory::Hotbar,
    /// The window menu (right click on a window), while open.
    pub menu: Option<crate::render::menu::PanelMenu>,
    /// The key help box (Mod+K), while shown.
    pub help: Option<crate::inventory::Image>,
    /// A panel shown flat over the whole screen instead of the 3D world (Mod+F).
    pub fullscreen: Option<crate::input::fullscreen::Fullscreen>,
    /// The view gliding to / standing in front of the window in use.
    pub focus_view: Option<crate::input::focus_view::FocusView>,
    pub hud: crate::render::hud::Hud,

    /// Menus and tooltips. Popups belong to a parent surface, not to the window list.
    pub popups: PopupManager,
    /// 2D layout of windows and outputs. In 3D mode (Stage 3+) windows live in the world
    /// instead; the space then only tracks outputs.
    pub space: Space<Window>,
    /// Where new windows are placed in the 2D cascade (0..10).
    pub cascade_index: i32,

    /// Pointer position in global logical coordinates (2D mode).
    pub pointer_location: Point<f64, Logical>,
    /// What the client focused under the pointer asked the cursor to look like.
    pub cursor_status: CursorImageStatus,
    pub cursor_theme: CursorThemeCache,
    pub dnd_icon: Option<DndIcon>,

    /// 2D (flat debug view) or 3D.
    pub render_mode: RenderMode,
    /// Camera, scene renderer and stats for the 3D view.
    pub view: View3D,
    /// Windows as panels in 3D space.
    pub world: crate::world::World,
    /// 3D: Walk (camera) or Window (input goes to one panel).
    pub mode: crate::input::mode::InputMode,
    /// The panel under the crosshair (any distance; back hits included).
    pub hover: Option<crate::input::raycast::PanelHit>,
    /// Which panels have a border this frame.
    pub highlight: crate::input::mode::Highlight,
    /// Window mode: the cursor position on the focused panel, window-geometry px.
    pub virtual_cursor: Point<f64, Logical>,
    pub input_settings: crate::input::mode::InputSettings,
    pub crosshair: crate::render::crosshair::Crosshair,
    /// The user just started a program: its first window may take focus until then.
    pub expect_window_until: Option<Instant>,

    /// The block world and the mesher.
    pub voxels: crate::world::voxels::VoxelWorld,
    pub player: crate::world::player::Player,
    /// The block under the crosshair within reach (outline, break, place).
    pub target_block: Option<crate::input::raycast::VoxelHit>,
    /// A panel being carried (Mod+G).
    pub grab: Option<crate::input::world_ops::Grab>,
    /// Saved panel placements and block edits.
    pub layout: crate::world::layout::Layout,
    layout_timer: Option<smithay::reexports::calloop::RegistrationToken>,
    /// When physics last ran (for the time step).
    pub last_tick: Option<Instant>,
    pub debug_timer: Option<smithay::reexports::calloop::RegistrationToken>,

    // ---- Config (Stage 7.2) ----
    pub config: crate::config::Config,
    pub config_path: std::path::PathBuf,
    pub keybinds: crate::config::keys::Keybinds,
    /// Watches the config file; dropping it stops the watch.
    pub config_watcher: Option<notify::RecommendedWatcher>,
    pub config_reload_timer: Option<smithay::reexports::calloop::RegistrationToken>,
    /// Message box on screen (config errors).
    pub notice: Option<crate::render::notice::Notice>,
    /// Frame times and per-frame work, for `spacectl stats`.
    pub perf: crate::render::perf::Perf,
    /// When the last frame started (for `render.max_fps_idle`).
    pub last_frame: Option<Instant>,
    /// A delayed redraw waiting for the idle frame cap.
    pub capped_redraw: Option<smithay::reexports::calloop::RegistrationToken>,

    /// Input bookkeeping: suppressed key releases, modifier choice, etc.
    pub input: InputState,

    /// IPC control socket (spacectl).
    pub ipc: Option<IpcServer>,
    /// Screenshots waiting for the next rendered frame.
    pub pending_screenshots: Vec<ScreenshotRequest>,
    /// Worker threads report finished screenshots here.
    pub screenshot_tx: Sender<ScreenshotDone>,
}

impl SpaceState {
    pub fn new(
        display: &mut Display<SpaceState>,
        loop_handle: LoopHandle<'static, SpaceState>,
        loop_signal: LoopSignal,
        cli: Cli,
        backend: Backend,
        config: crate::config::Config,
        config_path: std::path::PathBuf,
    ) -> anyhow::Result<Self> {
        let dh = display.handle();

        // Globals are created in dependency order. Each one becomes visible to clients
        // through wl_registry as soon as it exists.
        let compositor_state = CompositorState::new::<Self>(&dh);
        tracing::debug!("global: wl_compositor, wl_subcompositor");
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let decoration_state =
            smithay::wayland::shell::xdg::decoration::XdgDecorationState::new::<Self>(&dh);
        let image_capture_source_state =
            smithay::wayland::image_capture_source::ImageCaptureSourceState::new();
        let output_capture_source_state =
            smithay::wayland::image_capture_source::OutputCaptureSourceState::new::<Self>(&dh);
        let image_copy_capture_state =
            smithay::wayland::image_copy_capture::ImageCopyCaptureState::new::<Self>(&dh);
        let wlr_screencopy_global = crate::wayland::screencopy::create_wlr_global(&dh);
        let idle_notifier_state =
            smithay::wayland::idle_notify::IdleNotifierState::new(&dh, loop_handle.clone());
        let idle_inhibit_state =
            smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<Self>(&dh);
        let viewporter_state = smithay::wayland::viewporter::ViewporterState::new::<Self>(&dh);
        let fractional_scale_state =
            smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<Self>(&dh);
        let relative_pointer_state =
            smithay::wayland::relative_pointer::RelativePointerManagerState::new::<Self>(&dh);
        let pointer_constraints_state =
            smithay::wayland::pointer_constraints::PointerConstraintsState::new::<Self>(&dh);
        let cursor_shape_state =
            smithay::wayland::cursor_shape::CursorShapeManagerState::new::<Self>(&dh);
        let activation_state =
            smithay::wayland::xdg_activation::XdgActivationState::new::<Self>(&dh);
        let presentation_state = smithay::wayland::presentation::PresentationState::new::<Self>(
            &dh,
            rustix::time::ClockId::Monotonic as u32,
        );
        let session_lock_state =
            smithay::wayland::session_lock::SessionLockManagerState::new::<Self, _>(&dh, |_| true);
        tracing::debug!("global: zwlr_layer_shell_v1");
        tracing::debug!("global: xdg_wm_base");
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        tracing::debug!("global: wl_shm");
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        tracing::debug!("global: wl_output manager + zxdg_output_manager_v1");

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");
        // The layout comes from `[input]` in the config. Empty fields mean "ask the
        // environment": xkbcommon then reads XKB_DEFAULT_LAYOUT / _VARIANT / _OPTIONS and
        // falls back to plain "us".
        let i = &config.input;
        let xkb = XkbConfig {
            layout: &i.keyboard_layout,
            variant: &i.keyboard_variant,
            options: (!i.keyboard_options.is_empty()).then(|| i.keyboard_options.clone()),
            ..XkbConfig::default()
        };
        tracing::info!(layout = i.keyboard_layout, options = i.keyboard_options, "keyboard");
        seat.add_keyboard(xkb, i.repeat_delay, i.repeat_rate)
            .map_err(|e| anyhow::anyhow!("failed to create keyboard: {e:?}"))?;
        seat.add_pointer();
        tracing::debug!("global: wl_seat 'seat0' (keyboard + pointer)");

        let data_device_state = DataDeviceState::new::<Self>(&dh);
        tracing::debug!("global: wl_data_device_manager");
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        tracing::debug!("global: zwp_primary_selection_device_manager_v1");
        let data_control_state =
            DataControlState::new::<Self, _>(&dh, Some(&primary_selection_state), |_| true);
        tracing::debug!("global: zwlr_data_control_manager_v1");

        let (screenshot_tx, screenshot_rx) = channel::channel();
        loop_handle
            .insert_source(screenshot_rx, |event, _, state| {
                if let channel::Event::Msg(done) = event {
                    crate::ipc::on_screenshot_done(state, done);
                }
            })
            .map_err(|e| anyhow::anyhow!("screenshot channel: {}", e.error))?;

        let render_mode = cli.render;
        let mut view = View3D::new(cli.shader_dir.clone());

        let layout_path = match &cli.layout {
            Some(path) => path.clone(),
            None => crate::logging::state_dir()?.join("layout.json"),
        };
        let layout = crate::world::layout::Layout::load(layout_path);
        // An explicit --world-seed wins over the config.
        let seed = cli.world_seed.unwrap_or(config.general.world_seed);
        let voxels = crate::world::voxels::VoxelWorld::new(seed, &layout.edits, &loop_handle)?;
        let player = crate::world::player::Player::new(voxels.spawn);
        view.camera.position = player.eye();
        view.camera.yaw = voxels.spawn_yaw;

        Ok(Self {
            backend,
            running: true,
            start_time: Instant::now(),
            cli,
            display_handle: dh,
            loop_handle,
            loop_signal,
            socket_name: OsString::new(),
            compositor_state,
            xdg_shell_state,
            xwayland_shell_state,
            xwayland: None,
            layer_shell_state,
            decoration_state,
            image_capture_source_state,
            output_capture_source_state,
            image_copy_capture_state,
            screencopy: Default::default(),
            session_lock_state,
            lock: Default::default(),
            idle_notifier_state,
            idle_inhibit_state,
            idle_inhibitors: Vec::new(),
            viewporter_state,
            fractional_scale_state,
            relative_pointer_state,
            pointer_constraints_state,
            cursor_shape_state,
            activation_state,
            presentation_state,
            wlr_screencopy_global,
            overlay: Overlay::None,
            launcher: None,
            desktop: Default::default(),
            inventory: None,
            hotbar: Default::default(),
            help: None,
            menu: None,
            fullscreen: None,
            focus_view: None,
            hud: Default::default(),
            shm_state,
            output_manager_state,
            seat_state,
            seat,
            data_device_state,
            primary_selection_state,
            data_control_state,
            popups: PopupManager::default(),
            space: Space::default(),
            cascade_index: 0,
            pointer_location: (0.0, 0.0).into(),
            cursor_status: CursorImageStatus::default_named(),
            cursor_theme: CursorThemeCache::load(),
            dnd_icon: None,
            render_mode,
            view,
            world: crate::world::World::default(),
            mode: Default::default(),
            hover: None,
            highlight: Default::default(),
            virtual_cursor: Point::default(),
            input_settings: Default::default(),
            crosshair: Default::default(),
            expect_window_until: None,
            voxels,
            player,
            target_block: None,
            grab: None,
            layout,
            layout_timer: None,
            last_tick: None,
            debug_timer: None,
            keybinds: crate::config::keys::Keybinds::new(&config.keybinds),
            config,
            config_path,
            config_watcher: None,
            config_reload_timer: None,
            notice: None,
            last_frame: None,
            perf: Default::default(),
            capped_redraw: None,
            input: InputState::default(),
            ipc: None,
            pending_screenshots: Vec::new(),
            screenshot_tx,
        })
    }

    /// Stop the compositor at the end of this loop iteration.
    pub fn quit(&mut self) {
        tracing::info!("quit requested");
        self.running = false;
        self.loop_signal.wakeup();
    }

    /// Something moves on its own (held keys, falling): keep rendering frames.
    pub fn animating(&self) -> bool {
        let now = Instant::now();
        self.render_mode == crate::cli::RenderMode::Scene3D
            && (self.view.controls.is_moving()
                || (!self.player.on_ground && !self.player.flying)
                || self.player.vel.length_squared() > 1e-6
                // A pulsing border (xdg-activation) is an animation too.
                || self.world.panels().iter().any(|p| p.attention_until.is_some_and(|t| t > now))
                || self.focus_view.is_some_and(|f| f.moving()))
    }

    /// Ask the backend for a new frame (something on screen changed).
    pub fn queue_redraw(&mut self) {
        self.backend.schedule_redraw();
    }

    /// A client committed new content. With `render.max_fps_idle` set and nothing else
    /// moving, such redraws are spaced out to that rate (a blinking cursor or an animated
    /// page then costs a fraction of the GPU time). Client frame callbacks follow our
    /// frames, so the apps themselves slow down too.
    pub fn queue_client_redraw(&mut self) {
        let cap = self.config.render.max_fps_idle;
        if cap == 0 || self.animating() {
            self.queue_redraw();
            return;
        }
        if self.capped_redraw.is_some() {
            return; // one delayed redraw is already waiting
        }
        let interval = std::time::Duration::from_secs_f64(1.0 / cap as f64);
        let wait = self
            .last_frame
            .map_or(std::time::Duration::ZERO, |t| interval.saturating_sub(t.elapsed()));
        if wait.is_zero() {
            self.queue_redraw();
            return;
        }
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        self.capped_redraw = self
            .loop_handle
            .insert_source(Timer::from_duration(wait), |_, _, state| {
                state.capped_redraw = None;
                state.queue_redraw();
                TimeoutAction::Drop
            })
            .ok();
    }

    /// The window whose surface tree contains `surface` (walks up from subsurfaces).
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        self.space.elements().find(|w| w.wl_surface().as_deref() == Some(&root)).cloned()
    }

    /// A client committed `surface`: flag the panel showing it so its texture is redrawn.
    /// Works for subsurfaces (walk up to the toplevel) and popups (walk to their parent).
    pub fn mark_surface_dirty(&mut self, surface: &WlSurface) {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if let Some(popup) = self.popups.find_popup(&root) {
            if let Ok(toplevel) = smithay::desktop::find_popup_root_surface(&popup) {
                root = toplevel;
            }
        }
        if let Some(panel) = self.world.panel_for_surface_mut(&root) {
            panel.content_dirty = true;
        }
    }

    /// How far in front of the camera a new window of `size_px` appears.
    pub fn spawn_distance(&self, size_px: (i32, i32)) -> f32 {
        if let crate::config::SpawnDistance::Metres(m) = self.config.panels.spawn_distance {
            return m;
        }
        let output =
            self.primary_output_geometry().map(|g| (g.size.w, g.size.h)).unwrap_or((1920, 1080));
        let ppu = self.world.default_ppu;
        let size_m = glam::Vec2::new(size_px.0 as f32, size_px.1 as f32) / ppu;
        crate::world::spawn_distance(size_m, output, ppu, self.view.camera.fov_y_deg)
    }

    /// Panels moved or blocks changed: save the layout 2 s from now (one save for a whole
    /// burst of changes, e.g. while carrying a panel around).
    pub fn layout_changed(&mut self) {
        self.layout.dirty = true;
        if self.layout_timer.is_some() {
            return;
        }
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        self.layout_timer = self
            .loop_handle
            .insert_source(
                Timer::from_duration(std::time::Duration::from_secs(2)),
                |_, _, state| {
                    state.layout_timer = None;
                    state.save_layout();
                    TimeoutAction::Drop
                },
            )
            .ok();
    }

    pub fn save_layout(&mut self) {
        if !self.layout.dirty {
            return;
        }
        let open = self.world.panels().iter().filter_map(saved_panel).collect();
        let apps = self.open_apps();
        self.layout.save(open, apps);
    }

    /// Copy a window's current app_id/title to its panel (see `Panel::names`).
    pub fn remember_names(&mut self, window: &Window) {
        let (app_id, title) = window_names(window);
        if let Some(panel) = self.world.panels_mut().iter_mut().find(|p| &p.window == window) {
            if app_id.is_some() {
                panel.names.0 = app_id;
            }
            if title.is_some() {
                panel.names.1 = title;
            }
        }
    }

    /// A panel's app is now known: put it where it was last time, if we remember.
    pub fn restore_placement(&mut self, window: &Window) {
        self.remember_names(window);
        let Some(panel) = self.world.panel_for_window(window) else {
            return;
        };
        if !panel.placement_pending {
            return;
        }
        let (app_id, title) = window_names(window);
        let Some(app_id) = app_id else {
            return;
        };
        let panel_id = panel.id;
        let mut saved = self.layout.take_placement(&app_id, title.as_deref());
        // A window the user just asked for (a key, the launcher) is entered right away. If
        // its saved spot is behind them or far off, they would type into a terminal they
        // can't see, with the mouse no longer turning the view: it looks frozen. Such a
        // window opens in front of them instead; the spot stays remembered.
        let requested = self.mode == (crate::input::mode::InputMode::Window { panel: panel_id })
            || self.expect_window_until.is_some_and(|until| std::time::Instant::now() < until);
        if requested && saved.as_ref().is_some_and(|s| !self.placement_in_view(s)) {
            tracing::info!(app_id, "saved placement out of view: opening in front instead");
            if let Some(s) = saved.take() {
                self.layout.remember(s);
            }
        }
        let Some(panel) = self.world.panels_mut().iter_mut().find(|p| &p.window == window) else {
            return;
        };
        panel.placement_pending = false;
        // A new app is open: the list of apps to reopen next time changed.
        self.layout_changed();
        let Some(panel) = self.world.panels_mut().iter_mut().find(|p| &p.window == window) else {
            return;
        };
        if let Some(saved) = saved {
            tracing::info!(app_id, "restored saved placement");
            panel.position = saved.position();
            panel.rotation = saved.rotation();
            panel.pixels_per_unit = saved.pixels_per_unit;
            panel.visible = !saved.hidden;
            self.view.dirty = true;
        }
    }

    /// Would a panel at this saved spot be seen from where the camera is now: in the view,
    /// front side towards us, close enough to read.
    fn placement_in_view(&self, saved: &crate::world::layout::SavedPanel) -> bool {
        /// Farther than this a fresh terminal is too small to read anyway.
        const MAX_DISTANCE: f32 = 8.0;
        let camera = &self.view.camera;
        let pos = saved.position();
        let to_camera = camera.position - pos;
        let frustum = crate::render::frustum::Frustum::from_matrix(
            camera.projection_matrix() * camera.view_matrix(),
        );
        tracing::debug!(
            camera = ?camera.position,
            ?pos,
            normal = ?(saved.rotation() * glam::Vec3::Z),
            in_frustum = frustum.sphere_visible(pos, 0.0),
            "placement view check"
        );
        !saved.hidden
            && to_camera.length() <= MAX_DISTANCE
            && (saved.rotation() * glam::Vec3::Z).dot(to_camera) > 0.0
            // Its middle, not just an edge, is on screen.
            && frustum.sphere_visible(pos, 0.0)
    }

    pub fn window_by_id(&self, id: u64) -> Option<Window> {
        self.space.elements().find(|w| window_id(w) == id).cloned()
    }

    /// The surface under a point in global coordinates, and that surface's origin
    /// in the same coordinates (what `pointer.motion` expects).
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        // Locked: only the lock surface of the output under the pointer.
        if self.lock.is_locked() {
            let output = self.space.outputs().find(|o| {
                self.space.output_geometry(o).is_some_and(|g| g.to_f64().contains(pos))
            })?;
            let origin = self.space.output_geometry(output)?.loc;
            let surface = self.lock.surface_for(output)?.wl_surface().clone();
            let (s, at) = smithay::desktop::utils::under_from_surface_tree(
                &surface,
                pos - origin.to_f64(),
                (0, 0),
                WindowSurfaceType::ALL,
            )?;
            return Some((PointerFocusTarget::Surface(s), (at + origin).to_f64()));
        }
        // Layer surfaces on top of windows first, then windows (only in 2D: in 3D they
        // are panels, reached through raycasts), then layers below them.
        use smithay::wayland::shell::wlr_layer::Layer;
        let windows = self.render_mode == crate::cli::RenderMode::Flat2D;
        if let Some(hit) = self.layer_surface_under(pos, &[Layer::Overlay, Layer::Top]) {
            return Some(hit);
        }
        let window_hit = if windows { self.space.element_under(pos) } else { None };
        let Some((window, window_pos)) = window_hit else {
            let below: &[Layer] =
                if windows { &[Layer::Bottom, Layer::Background] } else { &[Layer::Bottom] };
            return self.layer_surface_under(pos, below);
        };
        // `element_under` returns where the window's buffer origin is drawn (its position
        // minus the geometry offset, e.g. client-side shadows), which is the origin that
        // `surface_under` expects.
        window.surface_under(pos - window_pos.to_f64(), WindowSurfaceType::ALL).map(
            |(surface, surface_pos)| {
                // An X11 window's surface is reported as its wl_surface; route through the
                // X11Surface so focus handling stays X11-aware.
                let target = match window.x11_surface() {
                    Some(x11) if x11.wl_surface().as_ref() == Some(&surface) => {
                        PointerFocusTarget::X11(x11.clone())
                    }
                    _ => PointerFocusTarget::Surface(surface),
                };
                (target, (surface_pos + window_pos).to_f64())
            },
        )
    }

    /// The topmost layer surface (or one of its popups) at `pos` on the given layers.
    fn layer_surface_under(
        &self,
        pos: Point<f64, Logical>,
        layers: &[smithay::wayland::shell::wlr_layer::Layer],
    ) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        let output = self
            .space
            .outputs()
            .find(|o| self.space.output_geometry(o).is_some_and(|g| g.to_f64().contains(pos)))?;
        let origin = self.space.output_geometry(output)?.loc;
        let map = smithay::desktop::layer_map_for_output(output);
        let local = pos - origin.to_f64();
        for &layer in layers {
            let Some(surface) = map.layer_under(layer, local) else { continue };
            let Some(geo) = map.layer_geometry(surface) else { continue };
            if let Some((s, at)) =
                surface.surface_under(local - geo.loc.to_f64(), WindowSurfaceType::ALL)
            {
                return Some((PointerFocusTarget::Surface(s), (at + geo.loc + origin).to_f64()));
            }
        }
        None
    }

    /// Run after every event-loop dispatch: drop dead windows/popups and send everything
    /// we queued for clients. Without the flush, clients would only see our events when
    /// the next unrelated event happens.
    pub fn refresh_and_flush(&mut self) {
        self.space.refresh();
        self.popups.cleanup();
        if let Err(err) = self.display_handle.flush_clients() {
            tracing::warn!("failed to flush clients: {err}");
        }
    }
}

/// Per-client data. smithay stores one of these for every connected client.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, client_id: ClientId) {
        tracing::debug!(?client_id, "client initialized");
    }
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        tracing::info!(?client_id, ?reason, "client disconnected");
    }
}

/// Shorthand used by handlers that need an `Arc<ClientState>` for new clients.
pub fn new_client_state() -> Arc<ClientState> {
    Arc::new(ClientState::default())
}

/// Stable id of a window, shown by `spacectl list` and used by IPC requests.
#[derive(Debug, Clone, Copy)]
struct WindowId(u64);

pub fn window_id(window: &Window) -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    window.user_data().get_or_insert(|| WindowId(NEXT.fetch_add(1, Ordering::Relaxed))).0
}

/// app_id and title of a window (Wayland toplevel or X11).
pub fn window_names(window: &Window) -> (Option<String>, Option<String>) {
    if let Some(x11) = window.x11_surface() {
        return (Some(x11.class()), Some(x11.title()));
    }
    match window.wl_surface() {
        Some(s) => (
            crate::wayland::xdg_shell::toplevel_app_id(&s),
            crate::wayland::xdg_shell::toplevel_title(&s),
        ),
        None => (None, None),
    }
}

/// What gets saved about a panel (nothing for windows without an app_id: we couldn't
/// match them again anyway).
pub fn saved_panel(p: &crate::world::panel::Panel) -> Option<crate::world::layout::SavedPanel> {
    let (app_id, title) = window_names(&p.window);
    let app_id = app_id.or_else(|| p.names.0.clone());
    let title = title.or_else(|| p.names.1.clone());
    Some(crate::world::layout::SavedPanel {
        app_id: app_id?,
        title_hint: title,
        position: p.position.to_array(),
        rotation: p.rotation.to_array(),
        pixels_per_unit: p.pixels_per_unit,
        hidden: !p.visible,
    })
}
