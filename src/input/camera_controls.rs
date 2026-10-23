//! Walking: W/A/S/D move on the ground plane, Space goes up, Shift down.
//!
//! Only used in Walk mode (the key router never sends keys here in Window mode). Keys are
//! tracked by keycode so a release always matches its press, even if modifiers changed
//! the keysym in between.

use std::collections::HashMap;

use glam::Vec3;
use smithay::input::keyboard::{Keycode, Keysym};

use crate::render::camera::Camera;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Forward,
    Back,
    Left,
    Right,
    Up,
    Down,
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
        _ => return None,
    })
}

impl CameraControls {
    /// A movement key went down. Returns true if it is one.
    pub fn press(&mut self, keycode: Keycode, sym: Option<Keysym>) -> bool {
        match sym.and_then(movement_for) {
            Some(m) => {
                self.held.insert(keycode, m);
                true
            }
            None => false,
        }
    }

    /// A key went up. Returns true if it was one of our movement keys.
    pub fn release(&mut self, keycode: Keycode) -> bool {
        self.held.remove(&keycode).is_some()
    }

    pub fn is_moving(&self) -> bool {
        !self.held.is_empty()
    }

    /// Forget all held keys (mode change, focus loss): the camera must not drift.
    pub fn clear(&mut self) {
        self.held.clear();
    }

    /// Move the camera for `dt` seconds of held keys at `speed` m/s. True if it moved.
    pub fn update(&self, camera: &mut Camera, dt: f32, speed: f32) -> bool {
        let forward = camera.forward_flat();
        let right = forward.cross(Vec3::Y);
        let dir: Vec3 = self
            .held
            .values()
            .map(|m| match m {
                Move::Forward => forward,
                Move::Back => -forward,
                Move::Left => -right,
                Move::Right => right,
                Move::Up => Vec3::Y,
                Move::Down => Vec3::NEG_Y,
            })
            .sum();
        if dir.length_squared() < 1e-12 {
            return false; // nothing held, or opposite keys cancel out
        }
        // Normalize so diagonal movement isn't faster.
        camera.position += dir.normalize() * speed * dt;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEED: f32 = 4.0;

    #[test]
    fn press_and_release_match_by_keycode() {
        let mut c = CameraControls::default();
        let w = Keycode::new(25);
        assert!(c.press(w, Some(Keysym::w)));
        assert!(
            !c.press(Keycode::new(53), Some(Keysym::x)),
            "not a movement key"
        );
        assert!(c.is_moving());
        assert!(c.release(w));
        assert!(!c.is_moving());
    }

    #[test]
    fn forward_moves_along_flat_view_direction() {
        let mut c = CameraControls::default();
        c.press(Keycode::new(25), Some(Keysym::w));
        let mut cam = Camera {
            pitch: -1.0,
            ..Camera::default()
        };
        let start = cam.position;
        assert!(c.update(&mut cam, 0.5, SPEED));
        let moved = cam.position - start;
        assert!((moved.length() - SPEED * 0.5).abs() < 1e-4);
        assert!(
            moved.y.abs() < 1e-6,
            "looking down doesn't move you into the floor"
        );
        assert!(moved.z < 0.0, "yaw 0 walks toward -Z");
    }

    #[test]
    fn diagonal_is_not_faster_and_opposites_cancel() {
        let mut c = CameraControls::default();
        c.press(Keycode::new(25), Some(Keysym::w));
        c.press(Keycode::new(40), Some(Keysym::d));
        let mut cam = Camera::default();
        let start = cam.position;
        c.update(&mut cam, 1.0, SPEED);
        assert!(((cam.position - start).length() - SPEED).abs() < 1e-4);
        let mut c = CameraControls::default();
        c.press(Keycode::new(25), Some(Keysym::w));
        c.press(Keycode::new(39), Some(Keysym::s));
        assert!(!c.update(&mut cam, 1.0, SPEED));
    }
}
