//! Walking keys: W/A/S/D move, Space jumps (double tap: fly), Shift goes down while
//! flying, Ctrl sprints.
//!
//! Only used in Walk mode (the key router never sends keys here in Window mode). Keys are
//! tracked by keycode so a release always matches its press, even if modifiers changed
//! the keysym in between.

use std::collections::HashMap;

use smithay::input::keyboard::{Keycode, Keysym};

use crate::world::player::MoveIntent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Forward,
    Back,
    Left,
    Right,
    Up,
    Down,
    Sprint,
}

#[derive(Default)]
pub struct CameraControls {
    held: HashMap<Keycode, Move>,
}

pub fn movement_for(sym: Keysym) -> Option<Move> {
    Some(match sym {
        Keysym::w | Keysym::W => Move::Forward,
        Keysym::s | Keysym::S => Move::Back,
        Keysym::a | Keysym::A => Move::Left,
        Keysym::d | Keysym::D => Move::Right,
        Keysym::space => Move::Up,
        Keysym::Shift_L | Keysym::Shift_R => Move::Down,
        Keysym::Control_L | Keysym::Control_R => Move::Sprint,
        _ => return None,
    })
}

impl CameraControls {
    /// A movement key went down. Returns the movement if it is one.
    pub fn press(&mut self, keycode: Keycode, sym: Option<Keysym>) -> Option<Move> {
        let m = sym.and_then(movement_for)?;
        self.held.insert(keycode, m);
        Some(m)
    }

    /// A key went up. Returns true if it was one of our movement keys.
    pub fn release(&mut self, keycode: Keycode) -> bool {
        self.held.remove(&keycode).is_some()
    }

    /// Any key that moves you is held (Ctrl alone doesn't).
    pub fn is_moving(&self) -> bool {
        self.held.values().any(|m| *m != Move::Sprint)
    }

    /// Forget all held keys (mode change, focus loss): nothing must keep moving.
    pub fn clear(&mut self) {
        self.held.clear();
    }

    pub fn intent(&self) -> MoveIntent {
        let held = |m| self.held.values().any(|h| *h == m);
        let axis = |pos, neg| (held(pos) as i32 - held(neg) as i32) as f32;
        MoveIntent {
            forward: axis(Move::Forward, Move::Back),
            right: axis(Move::Right, Move::Left),
            up: held(Move::Up),
            down: held(Move::Down),
            sprint: held(Move::Sprint),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_and_release_match_by_keycode() {
        let mut c = CameraControls::default();
        let w = Keycode::new(25);
        assert_eq!(c.press(w, Some(Keysym::w)), Some(Move::Forward));
        assert_eq!(c.press(Keycode::new(53), Some(Keysym::x)), None, "not a movement key");
        assert!(c.is_moving());
        assert!(c.release(w));
        assert!(!c.is_moving());
    }

    #[test]
    fn intent_from_keys() {
        let mut c = CameraControls::default();
        c.press(Keycode::new(25), Some(Keysym::w));
        c.press(Keycode::new(38), Some(Keysym::a));
        c.press(Keycode::new(37), Some(Keysym::Control_L));
        let i = c.intent();
        assert_eq!((i.forward, i.right, i.sprint, i.up), (1.0, -1.0, true, false));
        // Opposite keys cancel.
        c.press(Keycode::new(39), Some(Keysym::s));
        assert_eq!(c.intent().forward, 0.0);
        // Ctrl alone doesn't count as moving (no continuous redraw for it).
        let mut c = CameraControls::default();
        c.press(Keycode::new(37), Some(Keysym::Control_L));
        assert!(!c.is_moving());
    }
}
