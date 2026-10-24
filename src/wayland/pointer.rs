//! Pointer extras for games and toolkits:
//!
//! - relative-pointer: raw mouse deltas (we already send them with every motion).
//! - pointer-constraints: a game *locks* the pointer (mouse-look) or *confines* it to its
//!   window. A lock is honored by freezing our virtual cursor on the panel while the raw
//!   deltas keep flowing to the game. It is only active while that surface has the
//!   pointer, so Mod+Escape always gets you out.
//! - cursor-shape: apps (GTK4, Qt 6) name a cursor ("text", "pointer") instead of
//!   uploading an image; our cursor code draws named shapes from the theme.

use smithay::{
    input::pointer::PointerHandle,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
    wayland::{
        pointer_constraints::{
            with_pointer_constraint, PointerConstraint, PointerConstraintsHandler,
        },
        seat::WaylandFocus,
    },
};

use crate::state::SpaceState;

impl PointerConstraintsHandler for SpaceState {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        tracing::debug!("pointer constraint requested");
        let focused = pointer.current_focus().and_then(|f| f.wl_surface().map(|s| s.into_owned()));
        if focused.as_ref() == Some(surface) {
            with_pointer_constraint(surface, pointer, |c| {
                if let Some(c) = c {
                    c.activate();
                }
            });
        }
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        // A game that locked the pointer says where its own cursor is drawn; when the lock
        // ends our cursor continues from there instead of jumping back.
        if let crate::input::mode::InputMode::Window { panel } = self.mode {
            let Some(p) = self.world.panels().iter().find(|p| p.id == panel) else { return };
            if p.window.wl_surface().as_deref() == Some(surface) {
                self.virtual_cursor = location - p.window.geometry().loc.to_f64();
            }
        }
    }
}

/// Tablet pens can also set cursor shapes (cursor-shape needs this); we have no tablet
/// support beyond that, so the defaults do.
impl smithay::input::tablet::TabletSeatHandler for SpaceState {
    type ToolFocus = WlSurface;
}

impl SpaceState {
    /// The pointer's current surface has an active pointer lock: don't move the cursor.
    pub fn pointer_locked(&self) -> bool {
        let Some(pointer) = self.seat.get_pointer() else { return false };
        let Some(surface) =
            pointer.current_focus().and_then(|f| f.wl_surface().map(|s| s.into_owned()))
        else {
            return false;
        };
        with_pointer_constraint(&surface, &pointer, |c| {
            c.is_some_and(|c| matches!(*c, PointerConstraint::Locked(_)) && c.is_active())
        })
    }

    /// After the pointer entered a surface: activate a constraint it asked for earlier.
    pub fn activate_pointer_constraint(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        let Some(surface) =
            pointer.current_focus().and_then(|f| f.wl_surface().map(|s| s.into_owned()))
        else {
            return;
        };
        with_pointer_constraint(&surface, &pointer, |c| {
            if let Some(c) = c {
                if !c.is_active() {
                    c.activate();
                }
            }
        });
    }
}
