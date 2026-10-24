//! Mod+D: switch between the 3D world and a plain 2D desktop while running. On the 2D
//! desktop, Mod+drag moves a window and Mod+right-drag resizes it (windows have no
//! title bars to grab: spacewm asks apps not to draw their own).
//!
//! The 2D desktop is always there underneath: every window also has a place in the 2D
//! "space" (where spacewm --render 2d shows it), so switching just changes what is drawn
//! and how input is routed. Nothing is closed or moved: going back to 3D, every panel is
//! where it was.

use std::time::{Duration, Instant};

use smithay::{
    input::pointer::MotionEvent,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use super::mode::{set_mode, InputMode};
use crate::{cli::RenderMode, state::SpaceState};

impl SpaceState {
    pub fn toggle_2d(&mut self) {
        let to = match self.render_mode {
            RenderMode::Scene3D => RenderMode::Flat2D,
            RenderMode::Flat2D => RenderMode::Scene3D,
        };
        tracing::info!(?to, "render mode switched");
        // Leave everything that only exists in 3D first (fullscreen, a window in use,
        // the view gliding, overlays with their own cursor, a carried panel).
        set_mode(self, InputMode::Walk, None);
        self.focus_view_cancel();
        if self.render_mode == RenderMode::Flat2D {
            // Windows get their 3D sizes back.
            self.leave_2d();
        }
        self.close_inventory();
        self.close_menu();
        self.close_help();
        self.set_dock_cursor(false);
        self.grab = None;
        self.drag = None;
        self.drag2d = None;
        self.view.controls.clear();
        self.render_mode = to;

        match to {
            RenderMode::Flat2D => {
                // An ordinary desktop: cursor in the middle, the top window has the keys.
                if let Some(geo) = self.primary_output_geometry() {
                    self.pointer_location = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
                }
                self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
                // Workspaces and tiling (desktop2d/), then the keyboard to a window there.
                self.enter_2d();
                let pos = self.pointer_location;
                let under = self.surface_under(pos);
                if let Some(pointer) = self.seat.get_pointer() {
                    let serial = SERIAL_COUNTER.next_serial();
                    let time = smithay::backend::input::InputTime::now();
                    pointer.motion(self, under, &MotionEvent { location: pos, serial, time });
                    pointer.frame(self);
                }
            }
            RenderMode::Scene3D => {
                // Walking; panel textures were not updated while hidden.
                self.set_keyboard_focus(None);
                for p in self.world.panels_mut() {
                    p.content_dirty = true;
                }
                self.view.dirty = true;
            }
        }
        // Nested: 3D keeps the mouse (it turns the view), 2D lets it move freely.
        if let crate::backend::Backend::Winit(b) = &mut self.backend {
            b.on_render_mode(to == RenderMode::Scene3D);
        }
        self.queue_redraw();
    }
}

/// At most this often a resize is sent while dragging.
const RESIZE_EVERY: Duration = Duration::from_millis(33);

/// A Mod+drag on the 2D desktop: the window floats and follows the mouse.
pub struct Drag2d {
    panel: crate::world::panel::PanelId,
    resize: bool,
    start_pointer: Point<f64, Logical>,
    start: crate::desktop2d::tiling::Rect,
    last_sent: Option<Instant>,
}

impl SpaceState {
    /// Mod+left / Mod+right press on the 2D desktop: start moving / resizing the window
    /// under the pointer (it becomes floating, like in other tiling WMs). Returns true if
    /// a drag started.
    pub fn drag2d_press(&mut self, resize: bool) -> bool {
        let pos = self.pointer_location;
        let Some(window) = self.space.element_under(pos).map(|(w, _)| w.clone()) else {
            return false;
        };
        let Some(panel) = self.world.panel_for_window(&window).map(|p| p.id) else { return false };
        let Some(start) = self.rect_2d(panel) else { return false };
        self.focus_window(Some(&window), SERIAL_COUNTER.next_serial());
        self.note_2d_focus();
        tracing::debug!(resize, "2d drag started");
        self.set_floating_rect(panel, start);
        self.drag2d = Some(Drag2d { panel, resize, start_pointer: pos, start, last_sent: None });
        true
    }

    fn drag2d_rect(&self, d: &Drag2d) -> crate::desktop2d::tiling::Rect {
        let delta: Point<i32, Logical> = (self.pointer_location - d.start_pointer).to_i32_round();
        let mut r = d.start;
        if d.resize {
            r.w = (r.w + delta.x).max(200);
            r.h = (r.h + delta.y).max(120);
        } else {
            r.x += delta.x;
            r.y += delta.y;
        }
        r
    }

    /// Pointer moved during a 2D drag (the pointer position is already updated).
    pub fn drag2d_motion(&mut self) {
        let Some(d) = self.drag2d.as_ref() else { return };
        // Moving is cheap; a resize makes the app redraw: a few times a second.
        if d.resize && d.last_sent.is_some_and(|t| t.elapsed() < RESIZE_EVERY) {
            return;
        }
        let (panel, r) = (d.panel, self.drag2d_rect(d));
        self.set_floating_rect(panel, r);
        if let Some(d) = self.drag2d.as_mut() {
            d.last_sent = Some(Instant::now());
        }
    }

    pub fn drag2d_release(&mut self) {
        if let Some(d) = self.drag2d.take() {
            let r = self.drag2d_rect(&d);
            self.set_floating_rect(d.panel, r);
            tracing::debug!("2d drag done");
        }
    }
}
