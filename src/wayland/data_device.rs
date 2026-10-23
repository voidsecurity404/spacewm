//! Clipboard and drag-and-drop.
//!
//! wl_data_device is copy/paste and DnD between apps: the source app offers MIME types,
//! the receiving app picks one and the data flows directly between the two through a
//! pipe — the compositor only brokers the offer. Primary selection is the same thing for
//! "select to copy, middle-click to paste". Data control lets clipboard tools like
//! wl-copy work without a focused window.

use smithay::{
    input::{
        dnd::{DnDGrab, DndGrabHandler, DndTarget, GrabType, Source},
        pointer::Focus,
        Seat,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Serial},
    wayland::selection::{
        data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler},
        primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
        wlr_data_control::{DataControlHandler, DataControlState},
        SelectionHandler,
    },
};

use crate::state::{DndIcon, SpaceState};

impl SelectionHandler for SpaceState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for SpaceState {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl PrimarySelectionHandler for SpaceState {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.primary_selection_state
    }
}

impl DataControlHandler for SpaceState {
    fn data_control_state(&mut self) -> &mut DataControlState {
        &mut self.data_control_state
    }
}

impl DndGrabHandler for SpaceState {
    fn dropped(
        &mut self,
        _target: Option<DndTarget<'_, Self>>,
        _validated: bool,
        _seat: Seat<Self>,
        _location: Point<f64, Logical>,
    ) {
        self.dnd_icon = None;
        self.queue_redraw();
    }

    fn cancelled(&mut self, _seat: Seat<Self>, _location: Point<f64, Logical>) {
        self.dnd_icon = None;
        self.queue_redraw();
    }
}

impl WaylandDndGrabHandler for SpaceState {
    /// A client started dragging something (its click is still held).
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        match type_ {
            GrabType::Pointer => {
                let Some(pointer) = seat.get_pointer() else {
                    source.cancel();
                    return;
                };
                let Some(start_data) = pointer.grab_start_data() else {
                    source.cancel();
                    return;
                };
                self.dnd_icon = icon.map(|surface| DndIcon { surface, offset: (0, 0).into() });
                let grab = DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
                pointer.set_grab(self, grab, serial, Focus::Keep);
            }
            // No touch input yet.
            GrabType::Touch => source.cancel(),
        }
    }
}
