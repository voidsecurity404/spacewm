//! Seated mode (Mod+S): like sitting at a desk with monitors all around. Every visible
//! window is placed on an arc around you at reading distance, all facing you, in the
//! same left-to-right order they had; walking is off (you only turn your head). New
//! windows join the arc. Mod+S again puts the windows back where they were.
//!
//! Windows glide to their places instead of jumping (`PanelGlide`).

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use glam::{Quat, Vec3};

use crate::{state::SpaceState, world::panel::PanelId};

const GLIDE: Duration = Duration::from_millis(450);
/// Space between neighbors on the arc (radians).
const GAP: f32 = 0.05;
/// The arc never wraps all the way around: past this, it moves farther out.
const MAX_SPAN: f32 = 1.7 * std::f32::consts::PI;
const MAX_RADIUS: f32 = 4.0;

#[derive(Default)]
pub struct Seated {
    pub active: bool,
    /// Where each window was before (put back when leaving).
    saved: HashMap<PanelId, (Vec3, Quat)>,
    /// Windows on their way to a new place.
    glides: Vec<PanelGlide>,
}

#[derive(Debug, Clone, Copy)]
struct PanelGlide {
    panel: PanelId,
    from: (Vec3, Quat),
    to: (Vec3, Quat),
    start: Instant,
}

/// Places on an arc: `widths` (metres) in order left to right, around `center_yaw`
/// (radians, yaw 0 = looking along -Z), at `radius` or farther if they don't fit.
/// Returns the yaw of each window's center and the radius used.
pub fn arc(widths: &[f32], center_yaw: f32, radius: f32) -> (Vec<f32>, f32) {
    let mut r = radius.max(0.3);
    let angles =
        |r: f32| -> Vec<f32> { widths.iter().map(|w| 2.0 * (w / 2.0 / r).atan()).collect() };
    let span = |a: &[f32]| a.iter().sum::<f32>() + GAP * a.len().saturating_sub(1) as f32;
    let mut a = angles(r);
    while span(&a) > MAX_SPAN && r < MAX_RADIUS {
        r *= 1.15;
        a = angles(r);
    }
    let total = span(&a);
    // Left to right means increasing yaw (turning right).
    let mut yaw = center_yaw - total / 2.0;
    let mut centers = Vec::with_capacity(a.len());
    for ang in a {
        centers.push(yaw + ang / 2.0);
        yaw += ang + GAP;
    }
    (centers, r)
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl SpaceState {
    /// Mod+S.
    pub fn toggle_seated(&mut self) {
        if self.render_mode != crate::cli::RenderMode::Scene3D {
            return;
        }
        if self.seated.active {
            self.leave_seated();
        } else {
            self.enter_seated();
        }
    }

    fn enter_seated(&mut self) {
        self.seated.active = true;
        self.seated.saved =
            self.world.panels().iter().map(|p| (p.id, (p.position, p.rotation))).collect();
        // Standing still from now on.
        self.view.controls.clear();
        tracing::info!(windows = self.seated.saved.len(), "seated mode on");
        self.arrange_seated();
    }

    fn leave_seated(&mut self) {
        self.seated.active = false;
        let saved = std::mem::take(&mut self.seated.saved);
        let now = Instant::now();
        let mut glides = Vec::new();
        for p in self.world.panels() {
            if let Some(&to) = saved.get(&p.id) {
                glides.push(PanelGlide {
                    panel: p.id,
                    from: (p.position, p.rotation),
                    to,
                    start: now,
                });
            }
        }
        tracing::info!("seated mode off");
        self.seated.glides = glides;
        self.layout_changed();
        self.queue_redraw();
    }

    /// Put every visible window on the arc around the view (also when one opens).
    pub fn arrange_seated(&mut self) {
        if !self.seated.active {
            return;
        }
        // Around where you sit and look, not where the view may be gliding.
        let (eye, view_yaw) = self.walk_view();
        // Keep the left-to-right order the windows have as seen from here.
        let rel_yaw = |pos: Vec3| {
            let d = pos - eye;
            let yaw = d.x.atan2(-d.z);
            (yaw - view_yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        };
        struct Spot {
            id: PanelId,
            rel_yaw: f32,
            width: f32,
            size_px: (i32, i32),
            now: (Vec3, Quat),
        }
        let mut panels: Vec<Spot> = self
            .world
            .panels()
            .iter()
            .filter(|p| p.visible)
            .map(|p| Spot {
                id: p.id,
                rel_yaw: rel_yaw(p.position),
                width: p.world_size().x,
                size_px: p.size_px,
                now: (p.position, p.rotation),
            })
            .collect();
        if panels.is_empty() {
            return;
        }
        panels.sort_by(|a, b| a.rel_yaw.total_cmp(&b.rel_yaw));
        // The distance where the biggest one is sharp / fits, for all.
        let radius = panels.iter().map(|p| self.spawn_distance(p.size_px)).fold(0.0f32, f32::max);
        let widths: Vec<f32> = panels.iter().map(|p| p.width).collect();
        let (centers, r) = arc(&widths, view_yaw, radius);
        let now = Instant::now();
        let mut glides = Vec::new();
        for (p, yaw) in panels.iter().zip(centers) {
            let pos = eye + Vec3::new(yaw.sin(), 0.0, -yaw.cos()) * r;
            let d = eye - pos;
            let rot = Quat::from_rotation_y(d.x.atan2(d.z));
            glides.push(PanelGlide { panel: p.id, from: p.now, to: (pos, rot), start: now });
        }
        tracing::debug!(count = glides.len(), radius = r, "seated arc");
        self.seated.glides = glides;
        self.queue_redraw();
    }

    /// Every frame: move gliding windows.
    pub fn apply_panel_glides(&mut self) {
        if self.seated.glides.is_empty() {
            return;
        }
        let mut done = true;
        for g in self.seated.glides.clone() {
            let t = g.start.elapsed().as_secs_f32() / GLIDE.as_secs_f32();
            done &= t >= 1.0;
            let e = ease(t);
            if let Some(p) = self.world.panels_mut().iter_mut().find(|p| p.id == g.panel) {
                p.position = g.from.0.lerp(g.to.0, e);
                p.rotation = g.from.1.slerp(g.to.1, e);
            }
        }
        self.view.dirty = true;
        if done {
            self.seated.glides.clear();
            self.layout_changed();
        }
    }

    pub fn panels_gliding(&self) -> bool {
        !self.seated.glides.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_is_centered_and_ordered() {
        let (c, r) = arc(&[1.0, 1.0, 1.0], 0.0, 1.0);
        assert_eq!(r, 1.0);
        assert!(c[0] < c[1] && c[1] < c[2], "left to right");
        assert!(c[1].abs() < 1e-6, "middle one straight ahead");
        assert!((c[0] + c[2]).abs() < 1e-6, "symmetric");
        // Neighbors don't overlap: their angular half-widths plus the gap fit between.
        let half = (0.5f32).atan();
        assert!(c[1] - c[0] >= 2.0 * half + GAP - 1e-5);
    }

    #[test]
    fn too_many_windows_move_farther_out() {
        let widths = vec![1.25; 12];
        let (c, r) = arc(&widths, 1.0, 0.7);
        assert!(r > 0.7);
        let span = c[11] - c[0];
        assert!(span < MAX_SPAN);
    }
}
