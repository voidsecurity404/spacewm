//! Input handling.
//!
//! Backends deliver smithay `InputEvent`s; `process_input_event` turns each into a call
//! to one of the `handle_*` functions below. IPC input injection (tests) calls the same
//! functions, so injected input behaves exactly like real input.
//!
//! Stage 1: keyboard/pointer forwarding and compositor keybinds.
//! Stage 2: temporary camera controls.
//! Stage 4: Walk/Window input modes and raycast focus from the screen center.

pub mod inject;

use std::collections::HashSet;

use smithay::{
    backend::input::InputTime,
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    input::{
        keyboard::{keysyms, FilterResult, Keycode, Keysym, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{focus::KeyboardFocusTarget, state::SpaceState};

#[derive(Default)]
pub struct InputState {
    /// Keys whose press we intercepted; their release is intercepted too.
    pub suppressed_keys: HashSet<Keycode>,
}

/// Something a keybind does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SpawnTerminal,
    CloseWindow,
    Quit,
}

/// Compositor keybinds. `modkey` is Super (or Alt when nested, because the host desktop
/// usually grabs Super combos before they reach us). Keysyms are the layout's base
/// (unshifted) symbols, so Mod+Shift+Q matches `q`, not `Q`.
pub fn keybind_action(modkey: bool, mods: &ModifiersState, sym: Option<Keysym>) -> Option<Action> {
    if !modkey {
        return None;
    }
    let sym = sym?;
    match (mods.shift, mods.ctrl, sym.raw()) {
        (false, false, keysyms::KEY_Return) => Some(Action::SpawnTerminal),
        (true, false, keysyms::KEY_q) => Some(Action::CloseWindow),
        (true, false, keysyms::KEY_e) => Some(Action::Quit),
        _ => None,
    }
}

pub fn process_input_event<B: InputBackend>(state: &mut SpaceState, event: InputEvent<B>) {
    match event {
        InputEvent::Keyboard { event } => {
            handle_key(state, event.key_code(), event.state(), Event::time(&event));
        }
        InputEvent::PointerMotion { event } => {
            handle_pointer_motion(
                state,
                event.delta(),
                event.delta_unaccel(),
                Event::time(&event),
            );
        }
        InputEvent::PointerMotionAbsolute { event } => {
            let Some(geo) = state.primary_output_geometry() else {
                return;
            };
            let pos = event.position_transformed(geo.size) + geo.loc.to_f64();
            handle_pointer_motion_absolute(state, pos, Event::time(&event));
        }
        InputEvent::PointerButton { event } => {
            handle_pointer_button(
                state,
                event.button_code(),
                event.state(),
                Event::time(&event),
            );
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

pub fn handle_key(state: &mut SpaceState, keycode: Keycode, key_state: KeyState, time: InputTime) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    let nested = state.backend.is_nested();
    let action = keyboard.input(
        state,
        keycode,
        key_state,
        serial,
        time,
        |state, mods, handle| {
            if key_state == KeyState::Released {
                if state.input.suppressed_keys.remove(&keycode) {
                    return FilterResult::Intercept(None);
                }
                return FilterResult::Forward;
            }
            let modkey = mods.logo || (nested && mods.alt);
            match keybind_action(modkey, mods, handle.raw_latin_sym_or_raw_current_sym()) {
                Some(action) => {
                    state.input.suppressed_keys.insert(keycode);
                    FilterResult::Intercept(Some(action))
                }
                None => FilterResult::Forward,
            }
        },
    );
    if let Some(Some(action)) = action {
        run_action(state, action);
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

pub fn handle_pointer_motion_absolute(
    state: &mut SpaceState,
    pos: Point<f64, Logical>,
    time: InputTime,
) {
    let pos = clamp_to_outputs(state, pos);
    state.pointer_location = pos;
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let under = state.surface_under(pos);
    let serial = SERIAL_COUNTER.next_serial();
    pointer.motion(
        state,
        under,
        &MotionEvent {
            location: pos,
            serial,
            time,
        },
    );
    pointer.frame(state);
    // The cursor moved, so the screen changed.
    state.queue_redraw();
}

/// Relative motion (real mice on DRM). Also forwarded as relative-pointer events so
/// games can use raw deltas.
pub fn handle_pointer_motion(
    state: &mut SpaceState,
    delta: Point<f64, Logical>,
    delta_unaccel: Point<f64, Logical>,
    time: InputTime,
) {
    let pos = clamp_to_outputs(state, state.pointer_location + delta);
    state.pointer_location = pos;
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let under = state.surface_under(pos);
    let serial = SERIAL_COUNTER.next_serial();
    pointer.motion(
        state,
        under.clone(),
        &MotionEvent {
            location: pos,
            serial,
            time,
        },
    );
    pointer.relative_motion(
        state,
        under,
        &RelativeMotionEvent {
            delta,
            delta_unaccel,
            time,
        },
    );
    pointer.frame(state);
    state.queue_redraw();
}

pub fn handle_pointer_button(
    state: &mut SpaceState,
    button: u32,
    button_state: ButtonState,
    time: InputTime,
) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    // Click-to-focus. Not while a grab (popup menu, drag) is active: the grab decides.
    if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
        let window = state
            .space
            .element_under(pointer.current_location())
            .map(|(w, _)| w.clone());
        state.focus_window(window.as_ref(), serial);
    }
    pointer.button(
        state,
        &ButtonEvent {
            button,
            state: button_state,
            serial,
            time,
        },
    );
    pointer.frame(state);
    state.queue_redraw();
}

pub fn handle_pointer_axis(state: &mut SpaceState, frame: AxisFrame) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    pointer.axis(state, frame);
    pointer.frame(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, ctrl: bool) -> ModifiersState {
        ModifiersState {
            shift,
            ctrl,
            ..Default::default()
        }
    }

    #[test]
    fn binds_need_the_mod_key() {
        let m = mods(false, false);
        assert_eq!(keybind_action(false, &m, Some(Keysym::Return)), None);
        assert_eq!(
            keybind_action(true, &m, Some(Keysym::Return)),
            Some(Action::SpawnTerminal)
        );
    }

    #[test]
    fn shift_binds() {
        assert_eq!(
            keybind_action(true, &mods(true, false), Some(Keysym::q)),
            Some(Action::CloseWindow)
        );
        assert_eq!(
            keybind_action(true, &mods(true, false), Some(Keysym::e)),
            Some(Action::Quit)
        );
        // Without shift, Mod+Q is not "close": it goes to the client.
        assert_eq!(
            keybind_action(true, &mods(false, false), Some(Keysym::q)),
            None
        );
        // Ctrl changes the combo.
        assert_eq!(
            keybind_action(true, &mods(true, true), Some(Keysym::q)),
            None
        );
    }
}
