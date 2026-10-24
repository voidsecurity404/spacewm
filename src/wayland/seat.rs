//! wl_seat: a group of input devices used by one person (keyboard, pointer, touch).
//!
//! The seat tracks which client has keyboard focus and which surface is under the
//! pointer, and forwards input events to them. Clients also use the seat to set their
//! cursor image.

use smithay::{
    input::{keyboard::LedState, pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    wayland::{
        pointer_constraints::PointerConstraintsHandler,
        seat::WaylandFocus,
        selection::{data_device::set_data_device_focus, primary_selection::set_primary_focus},
    },
};

use crate::{
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    state::SpaceState,
};

impl SeatHandler for SpaceState {
    type KeyboardFocus = KeyboardFocusTarget;
    type PointerFocus = PointerFocusTarget;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<SpaceState> {
        &mut self.seat_state
    }

    /// Keyboard focus moved. The clipboard follows it: only the focused client may read
    /// the selection (otherwise any background app could sniff what you copy).
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyboardFocusTarget>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|f| f.wl_surface()).and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
        let id = match focused {
            Some(KeyboardFocusTarget::Window(w)) => Some(crate::state::window_id(w)),
            _ => None,
        };
        crate::ipc::broadcast(self, crate::ipc::proto::Event::FocusChanged { id });
        if self.hud.visible {
            self.queue_redraw(); // the HUD shows the focused title
        }
    }

    /// A client asked for a cursor image: hidden, a named shape, or its own surface.
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
        self.queue_redraw();
    }

    fn led_state_changed(&mut self, _seat: &Seat<Self>, led_state: LedState) {
        self.backend.update_led_state(led_state);
    }
}

// Required by smithay's pointer focus machinery; constraint requests themselves are handled
// in Stage 7.5.
impl PointerConstraintsHandler for SpaceState {
    fn new_constraint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) {
    }

    fn cursor_position_hint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &smithay::input::pointer::PointerHandle<Self>,
        _location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
    }
}
