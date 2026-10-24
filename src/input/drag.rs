//! Moving and resizing windows with the mouse, like on any desktop, while walking:
//!
//! - drag a window by its top edge: it follows the view (as with Mod+G) and stays where
//!   you let go (on a wall if you aim at one);
//! - drag its bottom-right corner: the window itself gets bigger or smaller.
//!
//! A press there that doesn't move is still a plain click (it enters the window), so
//! the edges never get in the way. Looking at those spots shows a hint under the
//! crosshair.

use std::time::{Duration, Instant};

use smithay::utils::{Logical, Point};

use super::{raycast::PanelHit, world_ops};
use crate::{state::SpaceState, world::panel::PanelId};

/// Mouse travel (px) before a press becomes a drag.
const DRAG_START: f64 = 6.0;
/// At most this often a resize is sent to the app while dragging.
const RESIZE_EVERY: Duration = Duration::from_millis(33);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// The top edge: move.
    Move,
    /// The bottom-right corner: resize.
    Resize,
}

impl Zone {
    pub fn hint(self) -> &'static str {
        match self {
            Zone::Move => "drag: move",
            Zone::Resize => "drag: resize",
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
    if px.x >= w - corner && px.y >= h - corner {
        Some(Zone::Resize)
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
            if d.zone == Zone::Move {
                world_ops::grab_panel(self, d.panel);
            }
        }
        if d.zone == Zone::Move {
            // Carried like Mod+G: the view turns and the window follows.
            self.drag = Some(d);
            return false;
        }
        // Resize: mouse px -> window px at the window's distance (same scale as the
        // cursor on a panel), then ask the app, a few times per second at most.
        let factor = self.panel_px_per_screen_px(d.panel).unwrap_or(1.0);
        d.size.0 += delta.x * factor;
        d.size.1 += delta.y * factor;
        if d.last_sent.is_none_or(|t| t.elapsed() >= RESIZE_EVERY) {
            self.send_drag_resize(&mut d);
        }
        self.drag = Some(d);
        true
    }

    fn send_drag_resize(&mut self, d: &mut Drag) {
        if let Some(window) = self.window_by_id(d.panel) {
            let size = (d.size.0.round() as i32, d.size.1.round() as i32);
            world_ops::resize_window(&window, size);
            d.last_sent = Some(Instant::now());
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
            Zone::Resize => {
                self.send_drag_resize(&mut d);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(x: f64, y: f64) -> Point<f64, Logical> {
        Point::from((x, y))
    }

    #[test]
    fn zones() {
        let size = (1000, 700);
        assert_eq!(zone_at(px(500.0, 5.0), size), Some(Zone::Move));
        assert_eq!(zone_at(px(500.0, 300.0), size), None);
        assert_eq!(zone_at(px(990.0, 690.0), size), Some(Zone::Resize));
        // The top-right corner is the move edge, not resize.
        assert_eq!(zone_at(px(990.0, 5.0), size), Some(Zone::Move));
        assert_eq!(zone_at(px(-1.0, 5.0), size), None);
        assert_eq!(zone_at(px(5.0, 5.0), (0, 0)), None);
        // Small windows still get usable spots.
        assert_eq!(zone_at(px(150.0, 15.0), (300, 200)), Some(Zone::Move));
        assert_eq!(zone_at(px(280.0, 180.0), (300, 200)), Some(Zone::Resize));
    }
}
