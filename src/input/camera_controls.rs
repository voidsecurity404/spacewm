//! Temporary camera controls (Stage 2; Stage 4 replaces them with Walk mode).
//!
//! Only in 3D mode and only while the RIGHT mouse button is held, so left clicks still
//! reach windows. While held: mouse movement looks around, W/A/S/D move on the ground
//! plane, Space goes up, Shift down. These keys are not forwarded to clients.

use std::collections::HashMap;

use glam::Vec3;
use smithay::{
    input::keyboard::{Keycode, Keysym},
    utils::{Logical, Point},
};

use crate::render::camera::Camera;

/// Radians of rotation per pixel of mouse movement.
pub const MOUSE_SENSITIVITY: f32 = 0.0025;
/// Metres per second.
pub const MOVE_SPEED: f32 = 4.0;

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
    /// Right mouse button is held.
    pub looking: bool,
    /// Movement keys currently held, by keycode (so the release matches the press even if
    /// modifiers changed the keysym in between).
    held: HashMap<Keycode, Move>,
    /// Last absolute pointer position while looking (nested winit only gives absolute
    /// positions; deltas are computed from consecutive ones).
    last_abs: Option<Point<f64, Logical>>,
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
    pub fn start_looking(&mut self, at: Point<f64, Logical>) {
        self.looking = true;
        self.last_abs = Some(at);
    }

    pub fn stop_looking(&mut self) {
        self.looking = false;
        self.last_abs = None;
    }

    /// Delta since the last absolute position.
    pub fn absolute_delta(&mut self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let delta = self.last_abs.map(|last| pos - last).unwrap_or_default();
        self.last_abs = Some(pos);
        delta
    }

    /// A key press while looking. Returns true if we consumed it.
    pub fn press(&mut self, keycode: Keycode, sym: Option<Keysym>) -> bool {
        if !self.looking {
            return false;
        }
        match sym.and_then(movement_for) {
            Some(m) => {
                self.held.insert(keycode, m);
                true
            }
            None => false,
        }
    }

    /// A key release. Returns true if it releases one of our movement keys.
    pub fn release(&mut self, keycode: Keycode) -> bool {
        self.held.remove(&keycode).is_some()
    }

    pub fn is_moving(&self) -> bool {
        !self.held.is_empty()
    }

    /// Move the camera for `dt` seconds of held keys. Returns true if it moved.
    pub fn update(&self, camera: &mut Camera, dt: f32) -> bool {
        let mut dir = Vec3::ZERO;
        let forward = camera.forward_flat();
        let right = forward.cross(Vec3::Y);
        for m in self.held.values() {
            dir += match m {
                Move::Forward => forward,
                Move::Back => -forward,
                Move::Left => -right,
                Move::Right => right,
                Move::Up => Vec3::Y,
                Move::Down => Vec3::NEG_Y,
            };
        }
        if dir == Vec3::ZERO {
            return false;
        }
        // Normalize so diagonal movement isn't faster.
        camera.position += dir.normalize() * MOVE_SPEED * dt;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_only_captured_while_looking() {
        let mut c = CameraControls::default();
        let w = Keycode::new(25);
        assert!(!c.press(w, Some(Keysym::w)));
        c.start_looking((0.0, 0.0).into());
        assert!(c.press(w, Some(Keysym::w)));
        assert!(c.is_moving());
        // Release still matches after looking stopped (no stuck movement).
        c.stop_looking();
        assert!(c.release(w));
        assert!(!c.is_moving());
    }

    #[test]
    fn forward_moves_along_flat_view_direction() {
        let mut c = CameraControls::default();
        c.start_looking((0.0, 0.0).into());
        c.press(Keycode::new(25), Some(Keysym::w));
        let mut cam = Camera {
            pitch: -1.0,
            ..Camera::default()
        };
        let start = cam.position;
        assert!(c.update(&mut cam, 0.5));
        let moved = cam.position - start;
        assert!((moved.length() - MOVE_SPEED * 0.5).abs() < 1e-4);
        assert!(
            moved.y.abs() < 1e-6,
            "looking down doesn't move you into the floor"
        );
        assert!(moved.z < 0.0, "yaw 0 walks toward -Z");
    }

    #[test]
    fn diagonal_is_not_faster() {
        let mut c = CameraControls::default();
        c.start_looking((0.0, 0.0).into());
        c.press(Keycode::new(25), Some(Keysym::w));
        c.press(Keycode::new(40), Some(Keysym::d));
        let mut cam = Camera::default();
        let start = cam.position;
        c.update(&mut cam, 1.0);
        assert!(((cam.position - start).length() - MOVE_SPEED).abs() < 1e-4);
    }

    #[test]
    fn absolute_deltas() {
        let mut c = CameraControls::default();
        c.start_looking((100.0, 100.0).into());
        assert_eq!(
            c.absolute_delta((110.0, 95.0).into()),
            Point::from((10.0, -5.0))
        );
        assert_eq!(
            c.absolute_delta((110.0, 95.0).into()),
            Point::from((0.0, 0.0))
        );
    }
}
