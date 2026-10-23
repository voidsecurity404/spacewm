//! Synthetic input for tests (`spacectl input ...`).
//!
//! Disabled unless spacewm runs with `--allow-input-injection`: otherwise any process of
//! your user could type into your terminals through the IPC socket. Everything here goes
//! through the same `handle_*` functions as real devices.

use smithay::{
    backend::input::{Axis, AxisSource, ButtonState, InputTime, KeyState},
    input::{
        keyboard::{xkb, Keycode, Keysym},
        pointer::AxisFrame,
    },
    utils::{Logical, Point},
};

use super::{
    handle_key, handle_pointer_axis, handle_pointer_button, handle_pointer_motion,
    handle_pointer_motion_absolute,
};
use crate::state::SpaceState;

/// Linux evdev button codes (linux/input-event-codes.h).
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressAction {
    Press,
    Release,
    /// Press then release.
    Tap,
}

fn keycode_for(state: &SpaceState, sym: Keysym) -> Result<Keycode, String> {
    let keyboard = state.seat.get_keyboard().ok_or("no keyboard")?;
    keyboard
        .keycode_for_keysym(sym)
        .ok_or_else(|| format!("no key produces keysym {sym:?} in the current layout"))
}

/// Whether `sym` sits on the base (unshifted) level of `keycode`.
fn is_base_level(state: &mut SpaceState, keycode: Keycode, sym: Keysym) -> bool {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return true;
    };
    keyboard.with_xkb_state(state, |ctx| {
        ctx.xkb()
            .lock()
            .map(|xkb| xkb.raw_syms_for_key_in_layout(keycode, xkb.active_layout()).contains(&sym))
            .unwrap_or(true)
    })
}

fn modifier_sym(name: &str) -> Option<Keysym> {
    Some(match name.to_ascii_lowercase().as_str() {
        "shift" => Keysym::Shift_L,
        "ctrl" | "control" => Keysym::Control_L,
        "alt" => Keysym::Alt_L,
        "super" | "logo" | "mod4" | "win" => Keysym::Super_L,
        _ => return None,
    })
}

fn key(state: &mut SpaceState, keycode: Keycode, key_state: KeyState) {
    handle_key(state, keycode, key_state, InputTime::now());
}

/// "Alt+Shift+q", "Return", "ctrl+c". The last part is an xkb keysym name.
pub fn key_combo(state: &mut SpaceState, combo: &str, action: PressAction) -> Result<(), String> {
    let parts: Vec<&str> = combo.split('+').map(str::trim).collect();
    let (main, mods) = parts.split_last().ok_or("empty key combo")?;
    if main.is_empty() {
        return Err(format!("bad key combo '{combo}'"));
    }
    let mut mod_codes = Vec::new();
    for m in mods {
        let sym = modifier_sym(m).ok_or_else(|| format!("unknown modifier '{m}'"))?;
        mod_codes.push(keycode_for(state, sym)?);
    }
    let sym = xkb::keysym_from_name(main, xkb::KEYSYM_NO_FLAGS);
    let sym = if sym.raw() == 0 {
        xkb::keysym_from_name(main, xkb::KEYSYM_CASE_INSENSITIVE)
    } else {
        sym
    };
    if sym.raw() == 0 {
        return Err(format!("unknown key name '{main}'"));
    }
    let code = keycode_for(state, sym)?;

    if action != PressAction::Release {
        for &m in &mod_codes {
            key(state, m, KeyState::Pressed);
        }
        key(state, code, KeyState::Pressed);
    }
    if action != PressAction::Press {
        key(state, code, KeyState::Released);
        for &m in mod_codes.iter().rev() {
            key(state, m, KeyState::Released);
        }
    }
    Ok(())
}

/// Type text character by character, pressing Shift where the layout needs it.
pub fn type_text(state: &mut SpaceState, text: &str) -> Result<(), String> {
    let shift = keycode_for(state, Keysym::Shift_L)?;
    for c in text.chars() {
        let sym = match c {
            '\n' => Keysym::Return,
            '\t' => Keysym::Tab,
            c => xkb::utf32_to_keysym(c as u32),
        };
        let code = keycode_for(state, sym).map_err(|e| format!("can't type {c:?}: {e}"))?;
        let needs_shift = !is_base_level(state, code, sym);
        if needs_shift {
            key(state, shift, KeyState::Pressed);
        }
        key(state, code, KeyState::Pressed);
        key(state, code, KeyState::Released);
        if needs_shift {
            key(state, shift, KeyState::Released);
        }
    }
    Ok(())
}

/// Move to output-local logical coordinates on the primary output.
pub fn pointer_move_to(state: &mut SpaceState, x: f64, y: f64) -> Result<(), String> {
    let geo = state.primary_output_geometry().ok_or("no output")?;
    let pos = geo.loc.to_f64() + Point::<f64, Logical>::from((x, y));
    handle_pointer_motion_absolute(state, pos, InputTime::now());
    Ok(())
}

pub fn pointer_move_by(state: &mut SpaceState, dx: f64, dy: f64) {
    let d = Point::from((dx, dy));
    handle_pointer_motion(state, d, d, InputTime::now());
}

pub fn button(state: &mut SpaceState, button: u32, action: PressAction) {
    if action != PressAction::Release {
        handle_pointer_button(state, button, ButtonState::Pressed, InputTime::now());
    }
    if action != PressAction::Press {
        handle_pointer_button(state, button, ButtonState::Released, InputTime::now());
    }
}

/// Scroll by wheel notches (positive dy = down).
pub fn scroll(state: &mut SpaceState, dx: f64, dy: f64) {
    let mut frame = AxisFrame::new(InputTime::now()).source(AxisSource::Wheel);
    for (axis, v) in [(Axis::Horizontal, dx), (Axis::Vertical, dy)] {
        if v != 0.0 {
            frame = frame.value(axis, v * 15.0).v120(axis, (v * 120.0) as i32);
        }
    }
    handle_pointer_axis(state, frame);
}
