//! Input handling.
//!
//! Backends deliver input events; each becomes a call to one of the `handle_*`
//! functions below. IPC input injection (tests) calls the same functions, so injected
//! input behaves exactly like real input.
//!
//! In 2D mode input works like any desktop (Stage 1). In 3D it depends on the input mode
//! (mode.rs): Walk drives the camera, Window drives one panel's client.

pub mod camera_controls;
pub mod drag;
pub mod flat;
pub mod focus_view;
pub mod fullscreen;
pub mod inject;
pub mod mode;
pub mod raycast;
pub mod seated;
pub mod world_ops;

use std::collections::HashSet;

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent,
    },
    input::{
        keyboard::{keysyms, FilterResult, Keycode, Keysym, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{
    cli::RenderMode,
    config::keys::{Keybinds, Mods},
    focus::KeyboardFocusTarget,
    state::SpaceState,
};
use mode::InputMode;

#[derive(Default)]
pub struct InputState {
    /// Keys whose press we intercepted; their release is intercepted too, otherwise the
    /// client would see a release for a key it never saw pressed.
    pub suppressed_keys: HashSet<Keycode>,
    /// Same for mouse buttons (the click that enters a panel).
    pub suppressed_buttons: HashSet<u32>,
    /// Walk mode with an absolute pointer (tests): last position, to compute deltas.
    pub last_abs: Option<Point<f64, Logical>>,
    /// Touchpad scrolling in Walk mode: notches not yet turned into hotbar steps.
    pub scroll_accum: f64,
}

/// Something a keybind does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SpawnTerminal,
    CloseWindow,
    Quit,
    /// Log FPS / frame time / camera once per second.
    ToggleDebug,
    /// Recompile shaders from --shader-dir.
    ReloadShaders,
    /// Nested only: give the mouse back to the host desktop (Ctrl+Alt+G, like a VM).
    ReleasePointer,
    /// Pick up / put down a panel.
    Grab,
    RotatePanel,
    ResetPanelRotation,
    ScalePanelUp,
    ScalePanelDown,
    TogglePanelHidden,
    /// 0-based index into `Block::PLACEABLE` (Mod+1 = 0).
    SelectBlock(u8),
    /// Back to walking (3D).
    WalkMode,
    /// Run `[[keybinds.spawn]]` entry n.
    Spawn(usize),
    /// Open/close the built-in launcher.
    Launcher,
    /// Make the window itself bigger / smaller (not just the panel).
    ResizeGrow,
    ResizeShrink,
    ToggleHud,
    /// E while walking: the block inventory.
    Inventory,
    /// The target panel over the whole screen, or back.
    ToggleFullscreen,
    /// Turn to the next window (and enter it when it is within reach).
    NextWindow,
    /// Show / hide every key on screen.
    Help,
    /// Windows on an arc around you, no walking (and back).
    ToggleSeated,
    /// The 3D world or a plain 2D desktop.
    Toggle2d,
    /// Open/close the settings box.
    Settings,
    /// Show workspace 1..9 (3D with building on: hotbar block).
    Workspace(u8),
    MoveToWorkspace(u8),
    ToggleFloat,
    FocusDir(crate::desktop2d::tiling::Dir),
    MoveDir(crate::desktop2d::tiling::Dir),
    /// Lock the screen now.
    Lock,
    Screenshot,
    ScreenshotRegion,
    /// A media key (works while locked too).
    Hardware(crate::desktop::Hw),
}

/// Is `sym` the key that acts as Mod? (Nested with mod_key = super, Alt does too.)
fn is_mod_key(mod_key: crate::config::ModKey, nested: bool, sym: Option<Keysym>) -> bool {
    let Some(sym) = sym else { return false };
    let alt = matches!(sym, Keysym::Alt_L | Keysym::Alt_R);
    let sup = matches!(sym, Keysym::Super_L | Keysym::Super_R);
    match mod_key {
        crate::config::ModKey::Alt => alt,
        crate::config::ModKey::Super => sup || (nested && alt),
    }
}

/// The nested-only escape hatch: Ctrl+Alt+G gives the mouse back to the host desktop
/// (like a VM window). Fixed, not configurable, so it can't be lost to a config typo.
fn is_release_pointer(nested: bool, mods: &ModifiersState, sym: Option<Keysym>) -> bool {
    nested && mods.ctrl && mods.alt && !mods.shift && sym.map(|s| s.raw()) == Some(keysyms::KEY_g)
}

/// Where a key press goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRoute {
    /// Mod+Escape in 3D: back to Walk mode.
    EscapeToWalk,
    Bind(Action),
    /// Walk mode movement key.
    Camera,
    /// Walk mode: any other key is eaten.
    Swallow,
    /// To the focused client.
    Forward,
}

/// Decide what a key press does. Pure, so it can be unit tested.
///
/// Mod+Escape is checked FIRST and unconditionally — whatever the config says: if it ever
/// failed, the user would be trapped inside a window with no way back to walking.
pub fn route_key(
    render: RenderMode,
    mode: InputMode,
    binds: &Keybinds,
    mods: Mods,
    sym: Option<Keysym>,
) -> KeyRoute {
    let three_d = render == RenderMode::Scene3D;
    if three_d && mods.mod_ && sym == Some(Keysym::Escape) {
        return KeyRoute::EscapeToWalk;
    }
    match sym.and_then(|sym| binds.get(mods, sym)) {
        Some(Action::WalkMode) if three_d => return KeyRoute::EscapeToWalk,
        Some(Action::WalkMode) => {}
        Some(action) => return KeyRoute::Bind(action),
        None => {}
    }
    match (three_d, mode) {
        (false, _) => KeyRoute::Forward,
        (true, InputMode::Window { .. }) => KeyRoute::Forward,
        (true, InputMode::Walk) => {
            // Minecraft keys, only while walking (in a window they are ordinary typing):
            // E opens the inventory, 1..9 pick a hotbar slot.
            let plain = !(mods.mod_ || mods.ctrl || mods.alt || mods.super_);
            match sym.map(|s| s.raw()) {
                Some(keysyms::KEY_e | keysyms::KEY_E) if plain => {
                    return KeyRoute::Bind(Action::Inventory)
                }
                Some(d @ keysyms::KEY_1..=keysyms::KEY_9) if plain => {
                    return KeyRoute::Bind(Action::SelectBlock((d - keysyms::KEY_1) as u8))
                }
                _ => {}
            }
            match sym.and_then(camera_controls::movement_for) {
                Some(_) => KeyRoute::Camera,
                None => KeyRoute::Swallow,
            }
        }
    }
}

/// libinput (Stage 6) delivers smithay's generic input events.
#[allow(dead_code)] // used by the DRM backend (Stage 6)
pub fn process_input_event<B: InputBackend>(state: &mut SpaceState, event: InputEvent<B>) {
    match event {
        InputEvent::Keyboard { event } => {
            handle_key(state, event.key_code(), event.state(), Event::time(&event));
        }
        InputEvent::PointerMotion { event } => {
            handle_pointer_motion(state, event.delta(), event.delta_unaccel(), Event::time(&event));
        }
        InputEvent::PointerMotionAbsolute { event } => {
            let Some(geo) = state.primary_output_geometry() else {
                return;
            };
            let pos = event.position_transformed(geo.size) + geo.loc.to_f64();
            handle_pointer_motion_absolute(state, pos, Event::time(&event));
        }
        InputEvent::PointerButton { event } => {
            handle_pointer_button(state, event.button_code(), event.state(), Event::time(&event));
        }
        InputEvent::PointerAxis { event } => {
            let source = event.source();
            // Wheel mice report v120 steps (120 = one notch); 15 px per notch matches
            // what GTK/Firefox expect when no continuous value is given.
            let amount = |axis| {
                event
                    .amount(axis)
                    .unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0)
            };
            let mut frame = AxisFrame::new(Event::time(&event)).source(source);
            for axis in [Axis::Horizontal, Axis::Vertical] {
                let value = amount(axis);
                if value != 0.0 {
                    frame = frame.value(axis, value);
                    if let Some(v120) = event.amount_v120(axis) {
                        frame = frame.v120(axis, v120 as i32);
                    }
                }
                // Touchpad: a zero value marks the end of a scroll (kinetic scrolling).
                if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                    frame = frame.stop(axis);
                }
            }
            handle_pointer_axis(state, frame);
        }
        _ => {}
    }
}

/// What the keyboard filter decided, carried out after smithay's `input` returns.
enum KeyOutcome {
    Escape,
    Action(Action),
    Emergency,
    SwitchVt(i32),
    /// A key for the open launcher: its symbol with modifiers applied, and Ctrl.
    Launcher(Keysym, bool),
    /// Mod+Escape while a layer surface holds the keyboard.
    DismissLayer,
    /// A key while the inventory is open: its symbol, and whether it was Mod+Escape.
    Inventory(Keysym, bool),
    CloseHelp,
    /// A key while the window menu is open.
    Menu(Keysym),
    /// A key while the settings box is open.
    Settings(Keysym),
}

pub fn handle_key(state: &mut SpaceState, keycode: Keycode, key_state: KeyState, time: InputTime) {
    let _span = tracing::trace_span!("input").entered();
    state.input_activity();
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    let nested = state.backend.is_nested();
    let outcome = keyboard.input(state, keycode, key_state, serial, time, |state, mods, handle| {
        // The Mod key held while walking frees a cursor for the dock.
        let mod_key = state.config.general.mod_key;
        let is_mod = is_mod_key(mod_key, nested, handle.raw_latin_sym_or_raw_current_sym());
        if is_mod && key_state == KeyState::Released {
            state.set_dock_cursor(false);
        }
        if key_state == KeyState::Released {
            let ours = state.input.suppressed_keys.remove(&keycode);
            let camera = state.view.controls.release(keycode);
            return if ours || camera {
                FilterResult::Intercept(None)
            } else {
                FilterResult::Forward
            };
        }
        let sym = handle.raw_latin_sym_or_raw_current_sym();
        // Emergency exit, checked before ANYTHING else and in every mode: on a real
        // session this is the way out of a broken desktop.
        if mods.ctrl && mods.alt && sym == Some(Keysym::BackSpace) {
            state.input.suppressed_keys.insert(keycode);
            return FilterResult::Intercept(Some(KeyOutcome::Emergency));
        }
        // Ctrl+Alt+F1..F12: the keymap turns these into XF86Switch_VT_n.
        let vt = handle.modified_sym().raw();
        if !nested && (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12).contains(&vt) {
            state.input.suppressed_keys.insert(keycode);
            let n = (vt - keysyms::KEY_XF86Switch_VT_1 + 1) as i32;
            return FilterResult::Intercept(Some(KeyOutcome::SwitchVt(n)));
        }
        // Locked: every key goes to the locker (keyboard focus can only be its surface).
        // No keybinds — nothing may open, close or show anything behind the lock.
        if state.lock.is_locked() {
            // Media keys still work (turn the music down without unlocking); nothing
            // else may run behind the lock.
            let combo = Mods::from_state(mods, state.config.general.mod_key, nested);
            if let Some(Action::Hardware(hw)) = sym.and_then(|s| state.keybinds.get(combo, s)) {
                state.input.suppressed_keys.insert(keycode);
                return FilterResult::Intercept(Some(KeyOutcome::Action(Action::Hardware(hw))));
            }
            return FilterResult::Forward;
        }
        if is_release_pointer(nested, mods, sym) {
            state.input.suppressed_keys.insert(keycode);
            return FilterResult::Intercept(Some(KeyOutcome::Action(Action::ReleasePointer)));
        }
        let combo_mods = Mods::from_state(mods, state.config.general.mod_key, nested);
        let mod_escape = combo_mods.mod_ && sym == Some(Keysym::Escape);
        // The settings box takes the keys while open (its own key closes it too).
        if state.settings.is_some() {
            state.input.suppressed_keys.insert(keycode);
            let bound = sym.and_then(|s| state.keybinds.get(combo_mods, s));
            let sym = if bound == Some(Action::Settings) || mod_escape {
                Keysym::Escape
            } else {
                sym.unwrap_or(Keysym::NoSymbol)
            };
            return FilterResult::Intercept(Some(KeyOutcome::Settings(sym)));
        }
        // The window menu takes the keys while open.
        if state.menu.is_some() {
            state.input.suppressed_keys.insert(keycode);
            return FilterResult::Intercept(Some(KeyOutcome::Menu(
                sym.unwrap_or(Keysym::NoSymbol),
            )));
        }
        // The key help is on top of everything: Escape (or its own key) closes it, other
        // keys do nothing while it is shown.
        if state.help.is_some() {
            state.input.suppressed_keys.insert(keycode);
            let bound = sym.and_then(|s| state.keybinds.get(combo_mods, s));
            if sym == Some(Keysym::Escape) || bound == Some(Action::Help) {
                return FilterResult::Intercept(Some(KeyOutcome::CloseHelp));
            }
            return FilterResult::Intercept(None);
        }
        // The launcher takes every key while it is open.
        if state.launcher.is_some() {
            state.input.suppressed_keys.insert(keycode);
            let bound = sym.and_then(|s| state.keybinds.get(combo_mods, s));
            if mod_escape || bound == Some(Action::Launcher) {
                return FilterResult::Intercept(Some(KeyOutcome::Action(Action::Launcher)));
            }
            return FilterResult::Intercept(Some(KeyOutcome::Launcher(
                handle.modified_sym(),
                mods.ctrl,
            )));
        }
        // The inventory is open: its keys only (walking is paused).
        if state.inventory.is_some() {
            state.input.suppressed_keys.insert(keycode);
            return FilterResult::Intercept(Some(KeyOutcome::Inventory(
                sym.unwrap_or(Keysym::NoSymbol),
                mod_escape,
            )));
        }
        // An exclusive layer surface (fuzzel...) has the keyboard: everything goes to it,
        // except Mod+Escape, which takes the keyboard back — a stuck overlay must never
        // trap you.
        if matches!(state.overlay, crate::state::Overlay::Layer(_)) {
            if mod_escape {
                state.input.suppressed_keys.insert(keycode);
                return FilterResult::Intercept(Some(KeyOutcome::DismissLayer));
            }
            return FilterResult::Forward;
        }
        if is_mod
            && state.mode == InputMode::Walk
            && state.config.dock.enabled
            && pointer_is_3d(state)
        {
            state.set_dock_cursor(true);
        }
        let route = route_key(state.render_mode, state.mode, &state.keybinds, combo_mods, sym);
        if route != KeyRoute::Forward {
            state.input.suppressed_keys.insert(keycode);
        }
        match route {
            KeyRoute::EscapeToWalk => FilterResult::Intercept(Some(KeyOutcome::Escape)),
            KeyRoute::Bind(action) => FilterResult::Intercept(Some(KeyOutcome::Action(action))),
            KeyRoute::Camera => {
                state.input.suppressed_keys.remove(&keycode);
                if state.view.controls.press(keycode, sym) == Some(camera_controls::Move::Up) {
                    // Double-tap Space toggles flying.
                    state.player.space_pressed(std::time::Instant::now());
                }
                state.queue_redraw();
                FilterResult::Intercept(None)
            }
            KeyRoute::Swallow => FilterResult::Intercept(None),
            KeyRoute::Forward => FilterResult::Forward,
        }
    });
    match outcome {
        Some(Some(KeyOutcome::Escape)) => mode::set_mode(state, InputMode::Walk, None),
        Some(Some(KeyOutcome::Action(action))) => run_action(state, action),
        Some(Some(KeyOutcome::Emergency)) => {
            tracing::warn!("Ctrl+Alt+Backspace: emergency exit");
            state.quit();
        }
        Some(Some(KeyOutcome::SwitchVt(vt))) => state.backend.change_vt(vt),
        Some(Some(KeyOutcome::Launcher(sym, ctrl))) => state.launcher_key(sym, ctrl),
        Some(Some(KeyOutcome::DismissLayer)) => state.dismiss_layer(),
        Some(Some(KeyOutcome::Inventory(sym, mod_escape))) => inventory_key(state, sym, mod_escape),
        Some(Some(KeyOutcome::CloseHelp)) => state.close_help(),
        Some(Some(KeyOutcome::Menu(sym))) => state.menu_key(sym),
        Some(Some(KeyOutcome::Settings(sym))) => state.settings_key(sym),
        _ => {}
    }
}

pub fn run_action(state: &mut SpaceState, action: Action) {
    tracing::debug!(?action, "keybind");
    match action {
        Action::SpawnTerminal => crate::spawn::spawn_terminal(state),
        Action::CloseWindow => {
            let focus = state.seat.get_keyboard().and_then(|k| k.current_focus());
            if let Some(KeyboardFocusTarget::Window(window)) = focus {
                world_ops::close_window(&window);
            }
        }
        Action::Quit => state.quit(),
        Action::ToggleDebug => toggle_debug(state),
        Action::ReloadShaders => {
            state.view.request_shader_reload();
            state.queue_redraw();
        }
        Action::ReleasePointer => {
            if let crate::backend::Backend::Winit(b) = &mut state.backend {
                b.toggle_pointer_release();
            }
        }
        Action::Grab => world_ops::toggle_grab(state),
        Action::RotatePanel => world_ops::rotate_target(state, false),
        Action::ResetPanelRotation => world_ops::rotate_target(state, true),
        Action::ScalePanelUp => world_ops::scale_target(state, true),
        Action::ScalePanelDown => world_ops::scale_target(state, false),
        Action::ResizeGrow => world_ops::resize_target(state, true),
        Action::ResizeShrink => world_ops::resize_target(state, false),
        Action::TogglePanelHidden => world_ops::toggle_hidden(state),
        // Building is off unless switched on (general.building).
        Action::SelectBlock(i) if state.config.general.building => state.select_slot(i as usize),
        Action::Inventory if state.config.general.building => state.toggle_inventory(),
        Action::SelectBlock(_) | Action::Inventory => {}
        Action::ToggleFullscreen if state.render_mode == RenderMode::Flat2D => {
            state.toggle_fullscreen_2d(None);
        }
        Action::ToggleFullscreen => {
            let target = world_ops::target_panel(state);
            state.toggle_fullscreen(target);
        }
        Action::NextWindow => mode::next_window(state),
        Action::Help => state.toggle_help(),
        Action::Lock => state.lock_now(),
        Action::ToggleSeated => state.toggle_seated(),
        Action::Toggle2d => state.toggle_2d(),
        Action::Settings => state.toggle_settings(),
        Action::Workspace(n) => {
            if state.render_mode == RenderMode::Scene3D && state.config.general.building {
                state.select_slot(n as usize - 1);
            } else {
                state.switch_workspace(n);
            }
        }
        Action::MoveToWorkspace(n) => state.move_to_workspace(n),
        Action::ToggleFloat => state.toggle_float(),
        Action::FocusDir(d) => state.focus_dir(d),
        Action::MoveDir(d) => state.move_dir(d),
        Action::Screenshot => state.screenshot(false),
        Action::ScreenshotRegion => state.screenshot(true),
        Action::Hardware(hw) => state.hardware_key(hw),
        Action::WalkMode => mode::set_mode(state, InputMode::Walk, None),
        Action::Launcher => state.toggle_launcher(),
        Action::ToggleHud => {
            let visible = !state.hud.visible;
            state.set_hud_visible(visible);
        }
        Action::Spawn(i) => {
            if let Some(bind) = state.config.keybinds.spawn.get(i) {
                let command = bind.command.clone();
                state.expect_window_until =
                    Some(std::time::Instant::now() + mode::SPAWN_FOCUS_WINDOW);
                // Errors are shown by launch.
                let _ = state.launch(&command);
            }
        }
    }
    state.queue_redraw();
}

/// Keys while the inventory is open: E / Escape close it, 1..9 pick a slot.
fn inventory_key(state: &mut SpaceState, sym: Keysym, mod_escape: bool) {
    match sym.raw() {
        _ if mod_escape => state.close_inventory(),
        keysyms::KEY_e | keysyms::KEY_E | keysyms::KEY_Escape => state.close_inventory(),
        d @ keysyms::KEY_1..=keysyms::KEY_9 => state.select_slot((d - keysyms::KEY_1) as usize),
        _ => {}
    }
}

fn toggle_debug(state: &mut SpaceState) {
    use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
    let stats = &mut state.view.stats;
    stats.enabled = !stats.enabled;
    tracing::info!(enabled = stats.enabled, "debug overlay");
    if let Some(token) = state.debug_timer.take() {
        state.loop_handle.remove(token);
    }
    if stats.enabled {
        let second = std::time::Duration::from_secs(1);
        state.debug_timer = state
            .loop_handle
            .insert_source(Timer::from_duration(second), move |_, _, state| {
                let camera = state.view.camera;
                state.view.stats.report(&camera);
                if let crate::backend::Backend::Drm(drm) = &mut state.backend {
                    for (output, flips, missed, hz) in drm.take_stats() {
                        tracing::info!(output, flips, missed, hz = format!("{hz:.2}"), "drm");
                    }
                }
                TimeoutAction::ToDuration(second)
            })
            .ok();
    }
}

/// Release every pressed key (e.g. the host window lost focus mid-press).
pub fn release_all_keys(state: &mut SpaceState) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };
    for keycode in keyboard.pressed_keys() {
        handle_key(state, keycode, KeyState::Released, InputTime::now());
    }
    state.view.controls.clear();
}

/// Per-frame input work in 3D: walking (held keys) and the hover highlight.
pub fn before_frame(state: &mut SpaceState) {
    let _span = tracing::trace_span!("physics").entered();
    state.refresh_2d();
    state.refresh_hud();
    state.refresh_inventory();
    state.refresh_launching();
    state.refresh_menu();
    state.refresh_settings();
    state.refresh_dock();
    if state.render_mode != RenderMode::Scene3D {
        return;
    }
    // Pulsing borders (xdg-activation) change every frame.
    let now = std::time::Instant::now();
    if state.world.panels().iter().any(|p| p.attention_until.is_some_and(|t| t > now)) {
        state.view.dirty = true;
    }
    let eye = state.player.eye();
    state.voxels.dispatch_dirty(eye);

    // Physics time step. After an idle period (nothing animating, no frames), take one
    // frame's worth instead of the whole gap.
    let now = std::time::Instant::now();
    let dt = match state.last_tick {
        Some(last) if state.animating() => now.duration_since(last).as_secs_f32().min(0.1),
        _ => 1.0 / 60.0,
    };
    state.last_tick = Some(now);
    let intent = match state.mode {
        // Seated: only looking around.
        InputMode::Walk if !state.seated.active => state.view.controls.intent(),
        _ => Default::default(),
    };
    let forward = state.view.camera.forward_flat();
    state.player.walk_speed = state.input_settings.walk_speed;
    let voxels = &state.voxels;
    let moved =
        state.player.update(dt, intent, forward, &|c| voxels.solid(c), &|c| voxels.water(c));
    // Fell off the world: back to the spawn point.
    if state.player.pos.y < -40.0 {
        state.player = crate::world::player::Player::new(state.voxels.spawn);
    }
    // While the view is gliding to / sitting in front of a window, it is not at the
    // player's eyes (focus_view.rs).
    if state.focus_view.is_none() && (moved || state.view.camera.position != state.player.eye()) {
        state.view.camera.position = state.player.eye();
        state.view.dirty = true;
    }
    state.focus_view_apply();
    state.apply_panel_glides();
    world_ops::update_grab(state);
    state.update_drag_resize();
    mode::update_hover(state);
    // Needs this frame's hover.
    state.refresh_drag_hint();
}

/// Keep the pointer inside the union of all outputs.
fn clamp_to_outputs(state: &SpaceState, pos: Point<f64, Logical>) -> Point<f64, Logical> {
    let mut best: Option<(f64, Point<f64, Logical>)> = None;
    for output in state.space.outputs() {
        let Some(geo) = state.space.output_geometry(output) else {
            continue;
        };
        let geo = geo.to_f64();
        let clamped = Point::from((
            pos.x.clamp(geo.loc.x, geo.loc.x + geo.size.w - 1.0),
            pos.y.clamp(geo.loc.y, geo.loc.y + geo.size.h - 1.0),
        ));
        let dist = (clamped.x - pos.x).powi(2) + (clamped.y - pos.y).powi(2);
        if best.is_none_or(|(d, _)| dist < d) {
            best = Some((dist, clamped));
        }
    }
    best.map(|(_, p)| p).unwrap_or(pos)
}

fn look(state: &mut SpaceState, delta: Point<f64, Logical>) {
    if delta.x == 0.0 && delta.y == 0.0 {
        return;
    }
    let sensitivity = state.input_settings.mouse_sensitivity;
    let dy = if state.input_settings.invert_y { -delta.y } else { delta.y };
    state.view.camera.rotate(delta.x as f32, dy as f32, sensitivity);
    state.view.dirty = true;
    state.queue_redraw();
}

pub fn handle_pointer_motion_absolute(
    state: &mut SpaceState,
    pos: Point<f64, Logical>,
    time: InputTime,
) {
    state.input_activity();
    if state.settings.is_some() {
        state.settings_pointer(pos);
        return;
    }
    if pointer_is_3d(state) && state.menu.is_some() {
        state.menu_pointer(pos);
        return;
    }
    if pointer_is_3d(state) && state.dock.cursor && state.mode == InputMode::Walk {
        state.dock_pointer(pos);
        return;
    }
    if pointer_is_3d(state) {
        match state.mode {
            // The inventory has an ordinary cursor.
            InputMode::Walk if state.inventory.is_some() => state.inventory_pointer(pos),
            InputMode::Walk => {
                // An absolute device can only turn by how far it moved since last time.
                let delta = state.input.last_abs.map(|last| pos - last).unwrap_or_default();
                state.input.last_abs = Some(pos);
                if !state.drag_motion(delta) {
                    look(state, delta);
                }
            }
            InputMode::Window { .. } => mode::point_virtual_cursor(state, pos, time),
        }
        return;
    }
    let pos = clamp_to_outputs(state, pos);
    state.pointer_location = pos;
    if state.drag2d.is_some() {
        state.drag2d_motion();
        return;
    }
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let under = state.surface_under(pos);
    let serial = SERIAL_COUNTER.next_serial();
    pointer.motion(state, under, &MotionEvent { location: pos, serial, time });
    pointer.frame(state);
    // The cursor moved, so the screen changed.
    state.queue_redraw();
}

/// Relative motion (real mice on DRM, a locked pointer when nested). Also forwarded as
/// relative-pointer events so games can use raw deltas.
pub fn handle_pointer_motion(
    state: &mut SpaceState,
    delta: Point<f64, Logical>,
    delta_unaccel: Point<f64, Logical>,
    time: InputTime,
) {
    state.input_activity();
    if state.settings.is_some() {
        state.settings_pointer(state.pointer_location + delta);
        return;
    }
    if pointer_is_3d(state) && state.menu.is_some() {
        state.menu_pointer(state.pointer_location + delta);
        return;
    }
    if pointer_is_3d(state) && state.dock.cursor && state.mode == InputMode::Walk {
        state.dock_pointer(state.pointer_location + delta);
        return;
    }
    if pointer_is_3d(state) {
        match state.mode {
            InputMode::Walk if state.inventory.is_some() => {
                state.inventory_pointer(state.pointer_location + delta);
            }
            // Reading the key help: the view holds still.
            InputMode::Walk if state.help.is_some() => {}
            InputMode::Walk => {
                // Resizing by the corner uses the mouse; otherwise it turns the view (and
                // a window dragged by its top edge follows).
                if !state.drag_motion(delta) {
                    look(state, delta);
                }
            }
            InputMode::Window { .. } => {
                mode::move_virtual_cursor(state, delta, time);
                if let Some(pointer) = state.seat.get_pointer() {
                    let under = pointer.current_focus().map(|f| (f, Point::default()));
                    // Raw deltas for games; the focus location is irrelevant for them.
                    pointer.relative_motion(
                        state,
                        under,
                        &RelativeMotionEvent { delta, delta_unaccel, time },
                    );
                    pointer.frame(state);
                }
            }
        }
        return;
    }
    let pos = clamp_to_outputs(state, state.pointer_location + delta);
    state.pointer_location = pos;
    if state.drag2d.is_some() {
        state.drag2d_motion();
        return;
    }
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let under = state.surface_under(pos);
    let serial = SERIAL_COUNTER.next_serial();
    pointer.motion(state, under.clone(), &MotionEvent { location: pos, serial, time });
    pointer.relative_motion(state, under, &RelativeMotionEvent { delta, delta_unaccel, time });
    pointer.frame(state);
    state.queue_redraw();
}

/// The pointer works in 3D (camera, or the cursor on a panel) — unless a layer surface
/// has the keyboard, in which case an ordinary 2D cursor clicks layer surfaces.
fn pointer_is_3d(state: &SpaceState) -> bool {
    state.render_mode == RenderMode::Scene3D
        && !matches!(state.overlay, crate::state::Overlay::Layer(_))
        && !state.lock.is_locked()
}

/// Linux evdev codes of the mouse buttons.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// A click while walking:
/// - inventory open: pick the block under the cursor;
/// - carrying a panel: put it down;
/// - left: enter the panel under the crosshair, else break the block there;
/// - right: place the selected block against the face under the crosshair.
///
/// Mod+left / Mod+right always break / place, even with a panel in front.
fn walk_click(state: &mut SpaceState, button: u32) {
    if state.inventory.is_some() {
        if button == BTN_LEFT {
            state.inventory_click();
        }
        return;
    }
    if state.grab.is_some() {
        world_ops::toggle_grab(state);
        return;
    }
    let mods = state.seat.get_keyboard().map(|k| k.modifier_state()).unwrap_or_default();
    let modkey =
        Mods::from_state(&mods, state.config.general.mod_key, state.backend.is_nested()).mod_;
    match (modkey, button) {
        (true, BTN_LEFT) if state.config.general.building => world_ops::break_block(state),
        // Right click on a window: its menu. (`hover` is only set when no block is in
        // front of the window.)
        (false, BTN_RIGHT) if state.hover.is_some() => {
            if let Some(hit) = state.hover {
                state.open_menu(hit.panel);
            }
        }
        (_, BTN_RIGHT) if state.config.general.building => world_ops::place_block(state),
        // Enter the panel under the crosshair; no panel there: break the block instead
        // (`target_block` is only set when no panel is nearer than the block).
        (false, BTN_LEFT) if !mode::try_enter_hovered(state) && state.config.general.building => {
            world_ops::break_block(state)
        }
        _ => {}
    }
}

pub fn handle_pointer_button(
    state: &mut SpaceState,
    button: u32,
    button_state: ButtonState,
    time: InputTime,
) {
    state.input_activity();
    // Releasing a press held on a window's grab spot: a drag ends, or (no movement) it
    // was a plain click after all.
    if button_state == ButtonState::Released && button == BTN_LEFT && state.drag.is_some() {
        state.input.suppressed_buttons.remove(&button);
        if state.drag_release() == Some(true) {
            walk_click(state, BTN_LEFT);
        }
        state.queue_redraw();
        return;
    }
    // 2D: the end of a Mod+drag.
    if button_state == ButtonState::Released && state.drag2d.is_some() {
        state.input.suppressed_buttons.remove(&button);
        state.drag2d_release();
        return;
    }
    if button_state == ButtonState::Released && state.input.suppressed_buttons.remove(&button) {
        return;
    }
    // The settings box: a click changes a row (right button: back) or closes it.
    if state.settings.is_some() {
        if button_state == ButtonState::Pressed {
            state.input.suppressed_buttons.insert(button);
            state.settings_click(button == BTN_RIGHT);
        }
        return;
    }
    // The window menu: a click picks an entry or closes it (and goes nowhere else).
    if state.menu.is_some() {
        if button_state == ButtonState::Pressed {
            state.input.suppressed_buttons.insert(button);
            state.menu_click();
        }
        return;
    }
    // Mod+right click while using a window: its menu.
    if let (InputMode::Window { panel }, ButtonState::Pressed, BTN_RIGHT) =
        (state.mode, button_state, button)
    {
        let mods = state.seat.get_keyboard().map(|k| k.modifier_state()).unwrap_or_default();
        let nested = state.backend.is_nested();
        if pointer_is_3d(state)
            && Mods::from_state(&mods, state.config.general.mod_key, nested).mod_
        {
            state.input.suppressed_buttons.insert(button);
            state.open_menu(panel);
            return;
        }
    }
    // A click closes the key help (and goes nowhere else).
    if state.help.is_some() {
        if button_state == ButtonState::Pressed {
            state.input.suppressed_buttons.insert(button);
            state.close_help();
        }
        return;
    }
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    if pointer_is_3d(state) {
        if state.mode == InputMode::Walk {
            // In Walk mode buttons never reach clients (press AND release are ours).
            if button_state == ButtonState::Pressed {
                state.input.suppressed_buttons.insert(button);
                // Holding Mod: the click is for the dock (or nothing).
                if state.dock.cursor && state.dock.cursor_moved {
                    state.dock_click(button == BTN_RIGHT);
                    return;
                }
                let mods =
                    state.seat.get_keyboard().map(|k| k.modifier_state()).unwrap_or_default();
                let modkey = Mods::from_state(
                    &mods,
                    state.config.general.mod_key,
                    state.backend.is_nested(),
                )
                .mod_;
                // On a window's top edge / corner: maybe a drag (decided by moving).
                let plain_left = button == BTN_LEFT && !modkey;
                let free = state.grab.is_none() && state.inventory.is_none();
                if !(plain_left && free && state.drag_press()) {
                    walk_click(state, button);
                }
            }
            return;
        }
    } else if button_state == ButtonState::Pressed
        && !pointer.is_grabbed()
        && state.render_mode == RenderMode::Flat2D
        && !state.lock.is_locked()
    {
        // The top bar: workspace buttons (and nothing goes to the windows below it).
        if let Some(geo) = state.primary_output_geometry() {
            let local = pointer.current_location() - geo.loc.to_f64();
            if local.y < crate::desktop2d::bar::HEIGHT as f64
                && state.desktop2d.fullscreen.is_none()
            {
                state.input.suppressed_buttons.insert(button);
                state.bar_click(local.x);
                return;
            }
        }
        // Mod+drag moves, Mod+right-drag resizes the window under the pointer.
        let mods = state.seat.get_keyboard().map(|k| k.modifier_state()).unwrap_or_default();
        let nested = state.backend.is_nested();
        let modkey = Mods::from_state(&mods, state.config.general.mod_key, nested).mod_;
        if modkey
            && (button == BTN_LEFT || button == BTN_RIGHT)
            && state.drag2d_press(button == BTN_RIGHT)
        {
            state.input.suppressed_buttons.insert(button);
            return;
        }
        // 2D click-to-focus. Not while a grab (popup menu, drag) is active: the grab decides.
        let window = state.space.element_under(pointer.current_location()).map(|(w, _)| w.clone());
        state.focus_window(window.as_ref(), serial);
        state.note_2d_focus();
        state.desktop2d.bar.dirty = true;
    }
    pointer.button(state, &ButtonEvent { button, state: button_state, serial, time });
    pointer.frame(state);
    state.queue_redraw();
}

pub fn handle_pointer_axis(state: &mut SpaceState, frame: AxisFrame) {
    state.input_activity();
    if pointer_is_3d(state) && state.mode == InputMode::Walk {
        let notches = frame.v120.map(|(_, v)| v as f64 / 120.0).unwrap_or(frame.axis.1 / 15.0);
        if notches == 0.0 {
            return;
        }
        if state.grab.is_some() {
            // Scrolling while carrying a panel moves it nearer/farther.
            world_ops::grab_scroll(state, -notches);
        } else if state.config.general.building {
            // Otherwise it walks the hotbar (down = next slot). Touchpads send many small
            // steps: one slot per notch's worth.
            let acc = &mut state.input.scroll_accum;
            *acc += notches;
            let steps = acc.trunc();
            *acc -= steps;
            if steps != 0.0 {
                state.scroll_slot(steps as i32);
            }
        }
        return;
    }
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    pointer.axis(state, frame);
    pointer.frame(state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::keys::KeybindsConfig;

    const D3: RenderMode = RenderMode::Scene3D;
    const WIN: InputMode = InputMode::Window { panel: 1 };
    const MOD: Mods = Mods { mod_: true, shift: false, ctrl: false, alt: false, super_: false };
    const NONE: Mods = Mods { mod_: false, shift: false, ctrl: false, alt: false, super_: false };

    fn binds() -> Keybinds {
        Keybinds::new(&KeybindsConfig::default())
    }

    #[test]
    fn binds_need_the_mod_key() {
        let b = binds();
        assert_eq!(route_key(D3, WIN, &b, NONE, Some(Keysym::Return)), KeyRoute::Forward);
        assert_eq!(
            route_key(D3, WIN, &b, MOD, Some(Keysym::Return)),
            KeyRoute::Bind(Action::SpawnTerminal)
        );
    }

    #[test]
    fn escape_always_wins_in_3d() {
        let b = binds();
        // From inside a window, and even with other modifiers held.
        assert_eq!(route_key(D3, WIN, &b, MOD, Some(Keysym::Escape)), KeyRoute::EscapeToWalk);
        let all = Mods { mod_: true, shift: true, ctrl: true, alt: true, super_: true };
        assert_eq!(route_key(D3, WIN, &b, all, Some(Keysym::Escape)), KeyRoute::EscapeToWalk);
        // Even with every keybind removed by the config.
        let empty = Keybinds::default();
        assert_eq!(route_key(D3, WIN, &empty, MOD, Some(Keysym::Escape)), KeyRoute::EscapeToWalk);
        // Plain Escape goes to the window (vim users!).
        assert_eq!(route_key(D3, WIN, &b, NONE, Some(Keysym::Escape)), KeyRoute::Forward);
    }

    #[test]
    fn walk_mode_can_be_rebound() {
        let config = KeybindsConfig {
            walk_mode: crate::config::keys::Binding(vec!["Mod+W".into()]),
            ..Default::default()
        };
        let b = Keybinds::new(&config);
        assert_eq!(route_key(D3, WIN, &b, MOD, Some(Keysym::w)), KeyRoute::EscapeToWalk);
        // In 2D there is no walking: the combo is not eaten either.
        assert_eq!(
            route_key(RenderMode::Flat2D, InputMode::Walk, &b, MOD, Some(Keysym::w)),
            KeyRoute::Forward
        );
    }

    #[test]
    fn walk_mode_keeps_keys_away_from_clients() {
        let b = binds();
        let walk = InputMode::Walk;
        assert_eq!(route_key(D3, walk, &b, NONE, Some(Keysym::w)), KeyRoute::Camera);
        assert_eq!(route_key(D3, walk, &b, NONE, Some(Keysym::x)), KeyRoute::Swallow);
        // Plain Escape in Walk mode does nothing special and isn't forwarded.
        assert_eq!(route_key(D3, walk, &b, NONE, Some(Keysym::Escape)), KeyRoute::Swallow);
        // Binds still work while walking.
        assert_eq!(
            route_key(D3, walk, &b, MOD, Some(Keysym::Return)),
            KeyRoute::Bind(Action::SpawnTerminal)
        );
    }

    #[test]
    fn window_mode_forwards_everything_but_binds() {
        let b = binds();
        assert_eq!(route_key(D3, WIN, &b, NONE, Some(Keysym::w)), KeyRoute::Forward);
        let shift = Mods { shift: true, ..MOD };
        assert_eq!(
            route_key(D3, WIN, &b, shift, Some(Keysym::q)),
            KeyRoute::Bind(Action::CloseWindow)
        );
    }

    #[test]
    fn two_d_mode_has_no_escape_route() {
        assert_eq!(
            route_key(RenderMode::Flat2D, InputMode::Walk, &binds(), MOD, Some(Keysym::Escape)),
            KeyRoute::Forward
        );
    }

    #[test]
    fn nested_pointer_release() {
        let ca = ModifiersState { ctrl: true, alt: true, ..Default::default() };
        assert!(is_release_pointer(true, &ca, Some(Keysym::g)));
        assert!(!is_release_pointer(false, &ca, Some(Keysym::g)), "only when nested");
    }
}
