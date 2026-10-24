//! Focus zoom: entering a window glides the view in front of it, at the distance where
//! one window pixel is one screen pixel (or the whole window fits), so text is always
//! sharp. Leaving glides back to where you were standing, looking where you looked.
//!
//! Only the view moves: the player stays where it is, so walking resumes from the same
//! spot. While the view is "away", each frame overrides the camera that the player
//! physics just placed (`apply`).

use std::time::{Duration, Instant};

use glam::Vec3;

use crate::{state::SpaceState, world::panel::PanelId};

/// Glide length: quick enough not to wait for, slow enough to see where you went.
const GLIDE: Duration = Duration::from_millis(350);
/// Not farther than this much past the right distance, and looking at the window within
/// ANGLE_ENOUGH: don't move. Nearer is fine too: someone who walked up to read part of a
/// big window must not be pulled back.
const CLOSE_ENOUGH: f32 = 0.15;
const ANGLE_ENOUGH: f32 = 0.12;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct FocusView {
    from: Pose,
    to: Pose,
    start: Instant,
    /// Where walking left off (camera at the player's eyes).
    walk: Pose,
    leaving: bool,
}

/// Smooth start and stop.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `a` to `b` the short way round (yaw wraps at 2π).
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let d = (b - a + PI).rem_euclid(TAU) - PI;
    (a + d * t).rem_euclid(TAU)
}

pub fn lerp(a: Pose, b: Pose, t: f32) -> Pose {
    Pose {
        pos: a.pos.lerp(b.pos, t),
        yaw: lerp_angle(a.yaw, b.yaw, t),
        pitch: a.pitch + (b.pitch - a.pitch) * t,
    }
}

/// Yaw/pitch that look from `from` at `at` (yaw 0 looks along -Z, see Camera::forward).
pub fn look_at(from: Vec3, at: Vec3) -> (f32, f32) {
    let d = at - from;
    let yaw = d.x.atan2(-d.z).rem_euclid(std::f32::consts::TAU);
    let pitch = d.y.atan2(d.x.hypot(d.z));
    (yaw, pitch)
}

impl FocusView {
    fn progress(&self) -> f32 {
        self.start.elapsed().as_secs_f32() / GLIDE.as_secs_f32()
    }

    pub fn moving(&self) -> bool {
        self.progress() < 1.0
    }
}

impl SpaceState {
    fn camera_pose(&self) -> Pose {
        let c = &self.view.camera;
        Pose { pos: c.position, yaw: c.yaw, pitch: c.pitch }
    }

    /// Entering `panel`: glide in front of it (if not already there).
    pub fn focus_view_enter(&mut self, panel: PanelId) {
        if !self.config.panels.focus_zoom {
            return;
        }
        let Some(p) = self.world.panels().iter().find(|p| p.id == panel) else { return };
        let output =
            self.primary_output_geometry().map(|g| (g.size.w, g.size.h)).unwrap_or((1920, 1080));
        let size_m = p.world_size();
        let d = crate::world::spawn_distance(
            size_m,
            output,
            p.pixels_per_unit,
            self.view.camera.fov_y_deg,
        );
        let target_pos = p.position + p.normal() * d;
        let (yaw, pitch) = look_at(target_pos, p.position);
        let to = Pose { pos: target_pos, yaw, pitch };
        let now = self.camera_pose();
        // Keep the walking pose from before, if we were already away (switching windows).
        let walk = self.focus_view.map_or(now, |f| f.walk);
        let dist = now.pos.distance(p.position);
        let towards = (p.position - now.pos).normalize_or_zero();
        let close = dist <= d * (1.0 + CLOSE_ENOUGH)
            && self.view.camera.forward().angle_between(towards) < ANGLE_ENOUGH;
        if close && self.focus_view.is_none() {
            return;
        }
        tracing::debug!(panel, distance = d, "focus zoom in");
        self.focus_view =
            Some(FocusView { from: now, to, start: Instant::now(), walk, leaving: false });
        self.queue_redraw();
    }

    /// Leaving the window: glide back to where walking left off.
    pub fn focus_view_leave(&mut self) {
        let Some(f) = self.focus_view else { return };
        if f.leaving {
            return;
        }
        // The player may have moved (falling while in the window): back to its eyes.
        let walk = Pose { pos: self.player.eye(), ..f.walk };
        self.focus_view = Some(FocusView {
            from: self.camera_pose(),
            to: walk,
            start: Instant::now(),
            walk,
            leaving: true,
        });
        self.queue_redraw();
    }

    /// Where the player looks from when walking: the eyes, and the walking yaw (the view
    /// itself may be away, zoomed into a window or gliding back).
    pub fn walk_view(&self) -> (Vec3, f32) {
        let yaw = self.focus_view.map_or(self.view.camera.yaw, |f| f.walk.yaw);
        (self.player.eye(), yaw)
    }

    /// Drop the zoom at once (the view is about to be set by someone else).
    pub fn focus_view_cancel(&mut self) {
        if let Some(f) = self.focus_view.take() {
            let c = &mut self.view.camera;
            c.position = self.player.eye();
            c.yaw = f.walk.yaw;
            c.pitch = f.walk.pitch;
            self.view.dirty = true;
        }
    }

    /// Every frame, after the player physics placed the camera.
    pub fn focus_view_apply(&mut self) {
        let Some(f) = self.focus_view else { return };
        let t = f.progress();
        let pose = lerp(f.from, f.to, ease(t));
        let c = &mut self.view.camera;
        if c.position != pose.pos || c.yaw != pose.yaw || c.pitch != pose.pitch {
            c.position = pose.pos;
            c.yaw = pose.yaw;
            c.pitch = pose.pitch;
            self.view.dirty = true;
        }
        if f.leaving && t >= 1.0 {
            self.focus_view = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_take_the_short_way() {
        let a = 6.2; // just under 2π
        let b = 0.1;
        let mid = lerp_angle(a, b, 0.5);
        // Through 0, not back through π.
        assert!(!(1.0..5.0).contains(&mid), "{mid}");
    }

    #[test]
    fn look_at_matches_camera_forward() {
        let mut cam = crate::render::camera::Camera::default();
        let from = Vec3::new(1.0, 2.0, 3.0);
        let at = Vec3::new(-2.0, 2.5, -1.0);
        let (yaw, pitch) = look_at(from, at);
        cam.yaw = yaw;
        cam.pitch = pitch;
        let dir = (at - from).normalize();
        assert!(cam.forward().dot(dir) > 0.9999, "{:?} vs {dir:?}", cam.forward());
    }

    #[test]
    fn ease_and_lerp_end_exactly() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(2.0), 1.0);
        let a = Pose { pos: Vec3::ZERO, yaw: 0.5, pitch: 0.1 };
        let b = Pose { pos: Vec3::ONE, yaw: 1.5, pitch: -0.2 };
        let e = lerp(a, b, 1.0);
        assert!(e.pos.distance(b.pos) < 1e-6 && (e.yaw - b.yaw).abs() < 1e-6);
    }
}
