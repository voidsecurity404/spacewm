//! Moving and resizing windows with the mouse, like on any desktop, while walking:
//!
//! - drag a window by its top edge: it follows the view (as with Mod+G) and stays where
//!   you let go (on a wall if you aim at one);
//! - drag one of its bottom corners: the window itself gets bigger or smaller. The view
//!   stands still while you hold the corner (the mouse only resizes), the corner moves
//!   with the mouse and the opposite top corner stays put, so you see how big it gets.
//!
//! A press there that doesn't move is still a plain click (it enters the window), so
//! the edges never get in the way. Looking at those spots shows a hint under the
//! crosshair.

use std::time::{Duration, Instant};

use glam::Vec3;
use smithay::utils::{Logical, Point};

use super::{raycast::PanelHit, world_ops};
use crate::{state::SpaceState, world::panel::PanelId};

/// Mouse travel (px) before a press becomes a drag.
const DRAG_START: f64 = 6.0;
/// At most this often a resize is sent to the app while dragging.
const RESIZE_EVERY: Duration = Duration::from_millis(33);
/// After letting go, the fixed corner is held this long while the app catches up.
const SETTLE: Duration = Duration::from_millis(1500);
/// Smallest window a drag makes (logical px).
const MIN_SIZE: (f64, f64) = (240.0, 140.0);
/// Biggest window a drag makes, in screens: bigger than the screen is fine (step back to
/// see it all), but not so big that one fast flick makes a giant window.
const MAX_SCREENS: f64 = 2.0;

/// A resize in progress: the top corner that stays put (top-left when dragging the
/// bottom-right corner, top-right for the bottom-left one) and the window's axes.
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub panel: PanelId,
    fixed: Vec3,
    right: Vec3,
    up: Vec3,
    ppu: f32,
    left: bool,
}

impl Anchor {
    fn of<W>(p: &crate::world::panel::Panel<W>, left: bool) -> Self {
        let right = p.rotation * Vec3::X;
        let up = p.rotation * Vec3::Y;
        let size = p.world_size();
        let side = if left { 1.0 } else { -1.0 };
        let fixed = p.position + right * (side * size.x / 2.0) + up * (size.y / 2.0);
        Anchor { panel: p.id, fixed, right, up, ppu: p.pixels_per_unit, left }
    }

    /// The panel center for a window of `size_px` with this corner fixed.
    fn center(&self, size_px: (i32, i32)) -> Vec3 {
        let (w, h) = (size_px.0 as f32 / self.ppu, size_px.1 as f32 / self.ppu);
        let side = if self.left { -1.0 } else { 1.0 };
        self.fixed + self.right * (side * w / 2.0) - self.up * (h / 2.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// The top edge: move.
    Move,
    /// The bottom-right corner: resize.
    /// A bottom corner; `left`: the bottom-left one.
    Resize { left: bool },
}

impl Zone {
    pub fn hint(self) -> &'static str {
        match self {
            Zone::Move => "drag: move",
            Zone::Resize { .. } => "drag: resize",
        }
    }
}

/// Which grab spot `px` (window pixels) is on, for a window of `size`.
pub fn zone_at(px: Point<f64, Logical>, size: (i32, i32)) -> Option<Zone> {
    let (w, h) = (size.0 as f64, size.1 as f64);
    if w <= 0.0 || h <= 0.0 || px.x < 0.0 || px.y < 0.0 || px.x >= w || px.y >= h {
        return None;
    }
    let edge = (h * 0.05).clamp(22.0, 40.0);
    let corner = (w.min(h) * 0.08).clamp(36.0, 72.0);
    if px.y >= h - corner && (px.x >= w - corner || px.x < corner) {
        Some(Zone::Resize { left: px.x < corner })
    } else if px.y < edge {
        Some(Zone::Move)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Drag {
    pub panel: PanelId,
    pub zone: Zone,
    /// Mouse travel so far (before the drag starts).
    travel: f64,
    pub started: bool,
    /// Resize: window size when the drag started and the size asked for now.
    start_size: (i32, i32),
    size: (f64, f64),
    last_sent: Option<Instant>,
    anchor: Option<Anchor>,
    /// Resize: the size the mouse asks for (`size` is it within the limits).
    wanted: (f64, f64),
    /// Resize: `size` changed since it was last sent to the app.
    pending: bool,
}

impl SpaceState {
    /// The grab spot under the crosshair, if a window within reach is seen there.
    pub fn drag_zone_under_crosshair(&self) -> Option<(PanelHit, Zone)> {
        let reach = self.input_settings.reach_distance;
        let hit = self.hover.filter(|h| !h.from_back && h.t <= reach)?;
        let p = self.world.panels().iter().find(|p| p.id == hit.panel)?;
        Some((hit, zone_at(hit.surface_px, p.size_px)?))
    }

    /// Left press while walking: on a grab spot, wait to see whether it becomes a drag.
    /// Returns true if the press was taken.
    pub fn drag_press(&mut self) -> bool {
        let Some((hit, zone)) = self.drag_zone_under_crosshair() else { return false };
        let size =
            self.world.panels().iter().find(|p| p.id == hit.panel).map_or((0, 0), |p| p.size_px);
        self.drag = Some(Drag {
            panel: hit.panel,
            zone,
            travel: 0.0,
            started: false,
            start_size: size,
            size: (size.0 as f64, size.1 as f64),
            last_sent: None,
            anchor: None,
            wanted: (size.0 as f64, size.1 as f64),
            pending: false,
        });
        true
    }

    /// Mouse motion while a press is held on a grab spot. Returns true when the motion
    /// is used up here (resizing: the view must not turn).
    pub fn drag_motion(&mut self, delta: Point<f64, Logical>) -> bool {
        let Some(mut d) = self.drag else { return false };
        if !d.started {
            d.travel += delta.x.hypot(delta.y);
            if d.travel < DRAG_START {
                self.drag = Some(d);
                return true; // hold still until it is clearly a drag
            }
            d.started = true;
            tracing::info!(panel = d.panel, zone = ?d.zone, "drag started");
            match d.zone {
                Zone::Move => world_ops::grab_panel(self, d.panel),
                Zone::Resize { left } => {
                    d.anchor = self
                        .world
                        .panels()
                        .iter()
                        .find(|p| p.id == d.panel)
                        .map(|p| Anchor::of(p, left));
                }
            }
        }
        let Zone::Resize { left } = d.zone else {
            // Moving: the view turns and the window follows (like Mod+G).
            self.drag = Some(d);
            return false;
        };
        // Resizing: the view stands still and the mouse moves the corner. Mouse px ->
        // window px at the window's distance (the same scale the cursor has on a panel),
        // so the corner stays under where the mouse would be. Dragging the left corner
        // to the left makes the window wider.
        let factor = self.panel_px_per_screen_px(d.panel).unwrap_or(1.0);
        let side = if left { -1.0 } else { 1.0 };
        d.wanted.0 += side * delta.x * factor;
        d.wanted.1 += delta.y * factor;
        let max = self
            .primary_output_geometry()
            .map_or((1920.0, 1080.0), |g| (g.size.w as f64, g.size.h as f64));
        d.size = (
            d.wanted.0.clamp(MIN_SIZE.0, (max.0 * MAX_SCREENS).max(MIN_SIZE.0)),
            d.wanted.1.clamp(MIN_SIZE.1, (max.1 * MAX_SCREENS).max(MIN_SIZE.1)),
        );
        // Past a limit and back: the corner comes back right away, not after the overshoot.
        d.wanted = d.size;
        d.pending = true;
        if d.last_sent.is_none_or(|t| t.elapsed() >= RESIZE_EVERY) {
            self.send_drag_resize(&mut d);
        }
        self.drag = Some(d);
        true
    }

    /// Window pixels per screen pixel for a panel at its current distance.
    pub fn panel_px_per_screen_px(&self, panel: PanelId) -> Option<f64> {
        let p = self.world.panels().iter().find(|p| p.id == panel)?;
        let output_h = self.primary_output_geometry().map(|g| g.size.h).unwrap_or(1080) as f64;
        let camera = &self.view.camera;
        let distance = camera.position.distance(p.position).max(0.05) as f64;
        let screen_ppm =
            output_h / (2.0 * distance * (camera.fov_y_deg as f64 / 2.0).to_radians().tan());
        Some(p.pixels_per_unit as f64 / screen_ppm)
    }

    /// Every frame: while resizing (and shortly after), send a size the mouse asked for
    /// between rate-limited sends, and keep the fixed corner where it was.
    pub fn update_drag_resize(&mut self) {
        let live = self.drag.filter(|d| d.started && matches!(d.zone, Zone::Resize { .. }));
        if let Some(mut d) = live {
            if d.pending && d.last_sent.is_none_or(|t| t.elapsed() >= RESIZE_EVERY) {
                self.send_drag_resize(&mut d);
            }
            self.drag = Some(d);
        }
        let anchor = live.and_then(|d| d.anchor).or_else(|| {
            self.resize_settle.filter(|(_, until)| Instant::now() < *until).map(|(a, _)| a)
        });
        if self.resize_settle.is_some_and(|(_, until)| Instant::now() >= until) {
            self.resize_settle = None;
        }
        if let Some(a) = anchor {
            if let Some(p) = self.world.panels_mut().iter_mut().find(|p| p.id == a.panel) {
                let center = a.center(p.size_px);
                if p.position.distance(center) > 1e-5 {
                    p.position = center;
                    self.view.dirty = true;
                }
            }
        }
    }

    /// Something is still being resized: keep rendering.
    pub fn resizing(&self) -> bool {
        self.drag.is_some_and(|d| d.started && matches!(d.zone, Zone::Resize { .. }))
            || self.resize_settle.is_some()
    }

    fn send_drag_resize(&mut self, d: &mut Drag) {
        if let Some(window) = self.window_by_id(d.panel) {
            let size = (d.size.0.round() as i32, d.size.1.round() as i32);
            world_ops::resize_window(&window, size);
            d.last_sent = Some(Instant::now());
            d.pending = false;
            self.queue_redraw();
        }
    }

    /// Left release. `None`: no press on a grab spot was held. `Some(true)`: the press
    /// never moved, so it was an ordinary click. `Some(false)`: a drag ended.
    pub fn drag_release(&mut self) -> Option<bool> {
        let mut d = self.drag.take()?;
        if !d.started {
            return Some(true);
        }
        match d.zone {
            Zone::Move => {
                // Put it down where it is (on the wall it was snapped to).
                if self.grab.is_some_and(|g| g.panel == d.panel) {
                    world_ops::toggle_grab(self);
                }
            }
            Zone::Resize { .. } => {
                self.send_drag_resize(&mut d);
                // The app answers the last size a moment later: hold the corner till then.
                if let Some(a) = d.anchor {
                    self.resize_settle = Some((a, Instant::now() + SETTLE));
                }
                self.layout_changed();
                tracing::info!(
                    panel = d.panel,
                    from = ?d.start_size,
                    to = ?(d.size.0.round() as i32, d.size.1.round() as i32),
                    "drag resize done"
                );
            }
        }
        Some(false)
    }

    /// Before each frame: the hint under the crosshair for the grab spot looked at.
    pub fn refresh_drag_hint(&mut self) {
        let shown = (self.mode == super::mode::InputMode::Walk
            && self.grab.is_none()
            && self.menu.is_none()
            && self.inventory.is_none()
            && self.help.is_none())
        .then(|| self.drag_zone_under_crosshair().map(|(_, z)| z))
        .flatten()
        // While dragging: keep showing what is being done.
        .or(self.drag.filter(|d| d.started).map(|d| d.zone));
        match (shown, &self.drag_hint) {
            (None, None) => {}
            (None, Some(_)) => {
                self.drag_hint = None;
                self.view.dirty = true;
            }
            (Some(z), Some((drawn, _))) if *drawn == z => {}
            (Some(z), _) => {
                tracing::debug!(hint = z.hint(), "drag hint");
                let scale = self
                    .space
                    .outputs()
                    .next()
                    .map(|o| o.current_scale().fractional_scale().ceil() as i32)
                    .unwrap_or(1)
                    .max(1);
                let style = crate::render::text::Style {
                    size: 13.0,
                    fg: [1.0, 1.0, 1.0, 1.0],
                    bg: [0.05, 0.06, 0.08, 0.75],
                    padding: 5,
                    max_width: 400,
                };
                self.drag_hint = crate::render::text::render(&[z.hint().to_string()], style, scale)
                    .map(|img| {
                        (z, (img.buffer(), (img.width / img.scale, img.height / img.scale)))
                    });
                self.view.dirty = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(x: f64, y: f64) -> Point<f64, Logical> {
        Point::from((x, y))
    }

    #[test]
    fn anchor_keeps_the_opposite_top_corner() {
        // A 800x400 px window at 800 px/m: 1 m x 0.5 m, facing +Z at the origin.
        let p = crate::world::panel::Panel::new(1, (), (800, 400));
        let a = Anchor::of(&p, false);
        assert!(a.fixed.distance(Vec3::new(-0.5, 0.25, 0.0)) < 1e-6, "{:?}", a.fixed);
        // Twice the size: the center moves right/down so the top-left stays.
        assert!(a.center((1600, 800)).distance(Vec3::new(0.5, -0.25, 0.0)) < 1e-6);
        // The bottom-left corner: the top-right stays, the window grows to the left.
        let b = Anchor::of(&p, true);
        assert!(b.fixed.distance(Vec3::new(0.5, 0.25, 0.0)) < 1e-6, "{:?}", b.fixed);
        assert!(b.center((1600, 800)).distance(Vec3::new(-0.5, -0.25, 0.0)) < 1e-6);
        // Same size: the center doesn't move.
        assert!(b.center((800, 400)).distance(Vec3::ZERO) < 1e-6);
    }

    #[test]
    fn zones() {
        let size = (1000, 700);
        assert_eq!(zone_at(px(500.0, 5.0), size), Some(Zone::Move));
        assert_eq!(zone_at(px(500.0, 300.0), size), None);
        assert_eq!(zone_at(px(990.0, 690.0), size), Some(Zone::Resize { left: false }));
        assert_eq!(zone_at(px(10.0, 690.0), size), Some(Zone::Resize { left: true }));
        assert_eq!(zone_at(px(500.0, 690.0), size), None, "the bottom edge in between");
        // The top-right corner is the move edge, not resize.
        assert_eq!(zone_at(px(990.0, 5.0), size), Some(Zone::Move));
        assert_eq!(zone_at(px(-1.0, 5.0), size), None);
        assert_eq!(zone_at(px(5.0, 5.0), (0, 0)), None);
        // Small windows still get usable spots.
        assert_eq!(zone_at(px(150.0, 15.0), (300, 200)), Some(Zone::Move));
        assert_eq!(zone_at(px(280.0, 180.0), (300, 200)), Some(Zone::Resize { left: false }));
    }
}
