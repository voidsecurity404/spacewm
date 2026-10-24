//! Input handling.
//!
//! Backends deliver input events; each becomes a call to one of the `handle_*`
//! functions below. IPC input injection (tests) calls the same functions, so injected
//! input behaves exactly like real input.
//!
//! In 2D mode input works like any desktop (Stage 1). In 3D it depends on the input mode
//! (mode.rs): Walk drives the camera, Window drives one panel's client.

pub mod camera_controls;
pub mod inject;
pub mod mode;
pub mod raycast;
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
    ToggleHud,
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
        (true, InputMode::Walk) => match sym.and_then(camera_controls::movement_for) {
            Some(_) => KeyRoute::Camera,
            None => KeyRoute::Swallow,
        },
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
}

pub fn handle_key(state: &mut SpaceState, keycode: Keycode, key_state: KeyState, time: InputTime) {
    state.input_activity();
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    let nested = state.backend.is_nested();
    let outcome = keyboard.input(state, keycode, key_state, serial, time, |state, mods, handle| {
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
            return FilterResult::Forward;
        }
        if is_release_pointer(nested, mods, sym) {
            state.input.suppressed_keys.insert(keycode);
            return FilterResult::Intercept(Some(KeyOutcome::Action(Action::ReleasePointer)));
        }
        let combo_mods = Mods::from_state(mods, state.config.general.mod_key, nested);
        let mod_escape = combo_mods.mod_ && sym == Some(Keysym::Escape);
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
                if let Some(toplevel) = window.toplevel() {
                    // Asks the app to close; it may show "unsaved changes?" first.
                    toplevel.send_close();
                } else if let Some(x11) = window.x11_surface() {
                    let _ = x11.close();
                }
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
        Action::TogglePanelHidden => world_ops::toggle_hidden(state),
        Action::SelectBlock(i) => world_ops::select_block(state, i as usize),
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
                if let Err(err) = crate::spawn::spawn(&command, &state.socket_name) {
                    tracing::warn!("{err}");
                }
            }
        }
    }
    state.queue_redraw();
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
    state.refresh_hud();
    if state.render_mode != RenderMode::Scene3D {
        return;
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
        InputMode::Walk => state.view.controls.intent(),
        InputMode::Window { .. } => Default::default(),
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
    if moved || state.view.camera.position != state.player.eye() {
        state.view.camera.position = state.player.eye();
        state.view.dirty = true;
    }
    world_ops::update_grab(state);
    mode::update_hover(state);
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
    if pointer_is_3d(state) {
        match state.mode {
            InputMode::Walk => {
                // An absolute device can only turn by how far it moved since last time.
                let delta = state.input.last_abs.map(|last| pos - last).unwrap_or_default();
                state.input.last_abs = Some(pos);
                look(state, delta);
            }
            InputMode::Window { .. } => mode::point_virtual_cursor(state, pos, time),
        }
        return;
    }
    let pos = clamp_to_outputs(state, pos);
    state.pointer_location = pos;
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
    if pointer_is_3d(state) {
        match state.mode {
            InputMode::Walk => look(state, delta),
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
/// - carrying a panel: put it down;
/// - Mod+left / Mod+right: break / place a block;
/// - left: enter the panel under the crosshair.
fn walk_click(state: &mut SpaceState, button: u32) {
    if state.grab.is_some() {
        world_ops::toggle_grab(state);
        return;
    }
    let mods = state.seat.get_keyboard().map(|k| k.modifier_state()).unwrap_or_default();
    let modkey =
        Mods::from_state(&mods, state.config.general.mod_key, state.backend.is_nested()).mod_;
    match (modkey, button) {
        (true, BTN_LEFT) => world_ops::break_block(state),
        (true, BTN_RIGHT) => world_ops::place_block(state),
        (false, BTN_LEFT) => {
            mode::try_enter_hovered(state);
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
    if button_state == ButtonState::Released && state.input.suppressed_buttons.remove(&button) {
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
                walk_click(state, button);
            }
            return;
        }
    } else if button_state == ButtonState::Pressed
        && !pointer.is_grabbed()
        && state.render_mode == RenderMode::Flat2D
        && !state.lock.is_locked()
    {
        // 2D click-to-focus. Not while a grab (popup menu, drag) is active: the grab decides.
        let window = state.space.element_under(pointer.current_location()).map(|(w, _)| w.clone());
        state.focus_window(window.as_ref(), serial);
    }
    pointer.button(state, &ButtonEvent { button, state: button_state, serial, time });
    pointer.frame(state);
    state.queue_redraw();
}

pub fn handle_pointer_axis(state: &mut SpaceState, frame: AxisFrame) {
    state.input_activity();
    if pointer_is_3d(state) && state.mode == InputMode::Walk {
        // Scrolling while carrying a panel moves it nearer/farther.
        let notches = frame.v120.map(|(_, v)| v as f64 / 120.0).unwrap_or(frame.axis.1 / 15.0);
        if notches != 0.0 {
            world_ops::grab_scroll(state, -notches);
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
