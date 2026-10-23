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

use crate::{cli::RenderMode, focus::KeyboardFocusTarget, state::SpaceState};
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
}

/// Compositor keybinds. `modkey` is Super (or Alt when nested, because the host desktop
/// usually grabs Super combos before they reach us). Keysyms are the layout's base
/// (unshifted) symbols, so Mod+Shift+Q matches `q`, not `Q`.
pub fn keybind_action(
    modkey: bool,
    nested: bool,
    mods: &ModifiersState,
    sym: Option<Keysym>,
) -> Option<Action> {
    let sym = sym?;
    if nested && mods.ctrl && mods.alt && !mods.shift && sym.raw() == keysyms::KEY_g {
        return Some(Action::ReleasePointer);
    }
    if !modkey {
        return None;
    }
    match (mods.shift, mods.ctrl, sym.raw()) {
        (false, false, keysyms::KEY_Return) => Some(Action::SpawnTerminal),
        (true, false, keysyms::KEY_q) => Some(Action::CloseWindow),
        (true, false, keysyms::KEY_e) => Some(Action::Quit),
        (false, false, keysyms::KEY_F3) => Some(Action::ToggleDebug),
        (false, false, keysyms::KEY_F5) => Some(Action::ReloadShaders),
        (false, false, keysyms::KEY_g) => Some(Action::Grab),
        (false, false, keysyms::KEY_r) => Some(Action::RotatePanel),
        (true, false, keysyms::KEY_r) => Some(Action::ResetPanelRotation),
        // "+" is Shift+"=" on most layouts; accept both, and the keypad.
        (_, false, keysyms::KEY_equal | keysyms::KEY_plus | keysyms::KEY_KP_Add) => {
            Some(Action::ScalePanelUp)
        }
        (false, false, keysyms::KEY_minus | keysyms::KEY_KP_Subtract) => {
            Some(Action::ScalePanelDown)
        }
        (false, false, keysyms::KEY_h) => Some(Action::TogglePanelHidden),
        (false, false, k @ keysyms::KEY_1..=keysyms::KEY_8) => {
            Some(Action::SelectBlock((k - keysyms::KEY_1) as u8))
        }
        _ => None,
    }
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
/// Mod+Escape is checked FIRST and unconditionally: if it ever failed, the user would be
/// trapped inside a window with no way back to walking.
pub fn route_key(
    render: RenderMode,
    mode: InputMode,
    modkey: bool,
    nested: bool,
    mods: &ModifiersState,
    sym: Option<Keysym>,
) -> KeyRoute {
    let three_d = render == RenderMode::Scene3D;
    if three_d && modkey && sym == Some(Keysym::Escape) {
        return KeyRoute::EscapeToWalk;
    }
    if let Some(action) = keybind_action(modkey, nested, mods, sym) {
        return KeyRoute::Bind(action);
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
}

pub fn handle_key(state: &mut SpaceState, keycode: Keycode, key_state: KeyState, time: InputTime) {
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
        let modkey = mods.logo || (nested && mods.alt);
        let sym = handle.raw_latin_sym_or_raw_current_sym();
        let route = route_key(state.render_mode, state.mode, modkey, nested, mods, sym);
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
    state.view.camera.rotate(delta.x as f32, delta.y as f32, sensitivity);
    state.view.dirty = true;
    state.queue_redraw();
}

pub fn handle_pointer_motion_absolute(
    state: &mut SpaceState,
    pos: Point<f64, Logical>,
    time: InputTime,
) {
    if state.render_mode == RenderMode::Scene3D {
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
    if state.render_mode == RenderMode::Scene3D {
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
    let modkey = mods.logo || (state.backend.is_nested() && mods.alt);
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
    if button_state == ButtonState::Released && state.input.suppressed_buttons.remove(&button) {
        return;
    }
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    if state.render_mode == RenderMode::Scene3D {
        if state.mode == InputMode::Walk {
            // In Walk mode buttons never reach clients (press AND release are ours).
            if button_state == ButtonState::Pressed {
                state.input.suppressed_buttons.insert(button);
                walk_click(state, button);
            }
            return;
        }
    } else if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
        // 2D click-to-focus. Not while a grab (popup menu, drag) is active: the grab decides.
        let window = state.space.element_under(pointer.current_location()).map(|(w, _)| w.clone());
        state.focus_window(window.as_ref(), serial);
    }
    pointer.button(state, &ButtonEvent { button, state: button_state, serial, time });
    pointer.frame(state);
    state.queue_redraw();
}

pub fn handle_pointer_axis(state: &mut SpaceState, frame: AxisFrame) {
    if state.render_mode == RenderMode::Scene3D && state.mode == InputMode::Walk {
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

    fn mods(shift: bool, ctrl: bool, alt: bool, logo: bool) -> ModifiersState {
        ModifiersState { shift, ctrl, alt, logo, ..Default::default() }
    }

    const D3: RenderMode = RenderMode::Scene3D;
    const WIN: InputMode = InputMode::Window { panel: 1 };

    #[test]
    fn binds_need_the_mod_key() {
        let m = mods(false, false, false, false);
        assert_eq!(keybind_action(false, false, &m, Some(Keysym::Return)), None);
        assert_eq!(
            keybind_action(true, false, &m, Some(Keysym::Return)),
            Some(Action::SpawnTerminal)
        );
    }

    #[test]
    fn shift_binds() {
        let shift = mods(true, false, false, true);
        assert_eq!(keybind_action(true, false, &shift, Some(Keysym::q)), Some(Action::CloseWindow));
        assert_eq!(keybind_action(true, false, &shift, Some(Keysym::e)), Some(Action::Quit));
        assert_eq!(
            keybind_action(true, false, &mods(false, false, false, true), Some(Keysym::q)),
            None
        );
        assert_eq!(
            keybind_action(true, false, &mods(true, true, false, true), Some(Keysym::q)),
            None
        );
    }

    #[test]
    fn escape_always_wins_in_3d() {
        let m = mods(false, false, false, true);
        // From inside a window, and even with other modifiers held.
        assert_eq!(
            route_key(D3, WIN, true, false, &m, Some(Keysym::Escape)),
            KeyRoute::EscapeToWalk
        );
        let all = mods(true, true, true, true);
        assert_eq!(
            route_key(D3, WIN, true, false, &all, Some(Keysym::Escape)),
            KeyRoute::EscapeToWalk
        );
        // Nested: Alt is the mod key.
        let alt = mods(false, false, true, false);
        assert_eq!(
            route_key(D3, WIN, true, true, &alt, Some(Keysym::Escape)),
            KeyRoute::EscapeToWalk
        );
        // Plain Escape goes to the window (vim users!).
        let none = mods(false, false, false, false);
        assert_eq!(
            route_key(D3, WIN, false, false, &none, Some(Keysym::Escape)),
            KeyRoute::Forward
        );
    }

    #[test]
    fn walk_mode_keeps_keys_away_from_clients() {
        let none = mods(false, false, false, false);
        let walk = InputMode::Walk;
        assert_eq!(route_key(D3, walk, false, false, &none, Some(Keysym::w)), KeyRoute::Camera);
        assert_eq!(route_key(D3, walk, false, false, &none, Some(Keysym::x)), KeyRoute::Swallow);
        // Plain Escape in Walk mode does nothing special and isn't forwarded.
        assert_eq!(
            route_key(D3, walk, false, false, &none, Some(Keysym::Escape)),
            KeyRoute::Swallow
        );
        // Binds still work while walking.
        let m = mods(false, false, false, true);
        assert_eq!(
            route_key(D3, walk, true, false, &m, Some(Keysym::Return)),
            KeyRoute::Bind(Action::SpawnTerminal)
        );
    }

    #[test]
    fn window_mode_forwards_everything_but_binds() {
        let none = mods(false, false, false, false);
        assert_eq!(route_key(D3, WIN, false, false, &none, Some(Keysym::w)), KeyRoute::Forward);
        let m = mods(true, false, false, true);
        assert_eq!(
            route_key(D3, WIN, true, false, &m, Some(Keysym::q)),
            KeyRoute::Bind(Action::CloseWindow)
        );
    }

    #[test]
    fn two_d_mode_has_no_escape_route() {
        let m = mods(false, false, false, true);
        assert_eq!(
            route_key(RenderMode::Flat2D, InputMode::Walk, true, false, &m, Some(Keysym::Escape)),
            KeyRoute::Forward
        );
    }

    #[test]
    fn nested_pointer_release() {
        let ca = mods(false, true, true, false);
        assert_eq!(keybind_action(false, true, &ca, Some(Keysym::g)), Some(Action::ReleasePointer));
        assert_eq!(keybind_action(false, false, &ca, Some(Keysym::g)), None, "only when nested");
    }
}
