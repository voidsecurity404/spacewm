//! Clipboard and drag-and-drop.
//!
//! wl_data_device is copy/paste and DnD between apps: the source app offers MIME types,
//! the receiving app picks one and the data flows directly between the two through a
//! pipe — the compositor only brokers the offer. Primary selection is the same thing for
//! "select to copy, middle-click to paste". Data control lets clipboard tools like
//! wl-copy work without a focused window.

use std::os::fd::OwnedFd;

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
        SelectionHandler, SelectionSource, SelectionTarget,
    },
};

use crate::state::{DndIcon, SpaceState};

impl SelectionHandler for SpaceState {
    type SelectionUserData = ();

    /// A Wayland app copied something: offer it to X11 apps too.
    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(wm) = self.xwayland.as_mut().and_then(|x| x.wm.as_mut()) {
            if let Err(e) = wm.new_selection(ty, source.map(|s| s.mime_types())) {
                tracing::warn!(?ty, "cannot offer the selection to X11: {e}");
            }
            // smithay queues the SetSelectionOwner request but doesn't flush it, so an X11
            // app pasting right away would still see the old owner. Any request that waits
            // for a reply flushes the queue; this one is cheap and has no side effects.
            let _ = wm.get_randr_primary_output();
        }
    }

    /// A Wayland app pastes what an X11 app copied: X11 writes it into `fd`.
    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _user_data: &(),
    ) {
        if let Some(wm) = self.xwayland.as_mut().and_then(|x| x.wm.as_mut()) {
            if let Err(e) = wm.send_selection(ty, mime_type, fd) {
                tracing::warn!(?ty, "cannot read the X11 selection: {e}");
            }
        }
    }
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
