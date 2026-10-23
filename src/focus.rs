//! Focus targets: WHO receives keyboard and pointer input.
//!
//! smithay lets the compositor choose its own focus types (`SeatHandler::KeyboardFocus`,
//! `SeatHandler::PointerFocus`). A plain `WlSurface` would work for pure Wayland apps, but
//! X11 windows (Stage 7.1) need extra work on focus — the X11 window manager must send
//! WM_TAKE_FOCUS / SetInputFocus — and smithay implements that on `X11Surface`. So we use
//! small enums that forward every call to the right inner target.

use std::{borrow::Cow, sync::Arc};

use smithay::{
    backend::input::{InputTime, KeyState},
    desktop::{LayerSurface, PopupKind, Window, WindowSurface},
    input::{
        dnd::{DndFocus, OfferData, Source},
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            PointerTarget, RelativeMotionEvent,
        },
        Seat,
    },
    reexports::wayland_server::{
        backend::ObjectId, protocol::wl_surface::WlSurface, DisplayHandle,
    },
    utils::{IsAlive, Logical, Point, Serial},
    wayland::{seat::WaylandFocus, selection::data_device::WlOfferData},
    xwayland::{xwm::XwmOfferData, X11Surface},
};

use crate::state::SpaceState;

/// Something that can hold keyboard focus.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Layer and Surface arrive in Stages 7.4/7.5.
pub enum KeyboardFocusTarget {
    /// An application window (Wayland xdg_toplevel or X11 window).
    Window(Window),
    /// A popup that grabbed the keyboard (menus).
    Popup(PopupKind),
    /// A layer-shell surface (launchers, lock screens; Stage 7.4).
    Layer(LayerSurface),
    /// A bare surface (session-lock surfaces; Stage 7.5).
    Surface(WlSurface),
}

/// Something that can hold pointer focus. Pointer events always go to one concrete
/// surface (a subsurface, a popup, ...), so this is finer-grained than keyboard focus.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum PointerFocusTarget {
    Surface(WlSurface),
    X11(X11Surface),
}

impl IsAlive for KeyboardFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Window(w) => w.alive(),
            Self::Popup(p) => p.alive(),
            Self::Layer(l) => l.alive(),
            Self::Surface(s) => s.alive(),
        }
    }
}

impl IsAlive for PointerFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Surface(s) => s.alive(),
            Self::X11(x) => x.alive(),
        }
    }
}

impl From<PopupKind> for KeyboardFocusTarget {
    fn from(popup: PopupKind) -> Self {
        Self::Popup(popup)
    }
}

impl From<Window> for KeyboardFocusTarget {
    fn from(window: Window) -> Self {
        Self::Window(window)
    }
}

impl From<WlSurface> for PointerFocusTarget {
    fn from(surface: WlSurface) -> Self {
        Self::Surface(surface)
    }
}

/// Popup grabs need to turn a keyboard focus into a pointer focus.
impl From<KeyboardFocusTarget> for PointerFocusTarget {
    fn from(target: KeyboardFocusTarget) -> Self {
        match target {
            KeyboardFocusTarget::Window(w) => match w.underlying_surface() {
                WindowSurface::Wayland(t) => Self::Surface(t.wl_surface().clone()),
                WindowSurface::X11(x) => Self::X11(x.clone()),
            },
            KeyboardFocusTarget::Popup(p) => Self::Surface(p.wl_surface().clone()),
            KeyboardFocusTarget::Layer(l) => Self::Surface(l.wl_surface().clone()),
            KeyboardFocusTarget::Surface(s) => Self::Surface(s),
        }
    }
}

impl WaylandFocus for KeyboardFocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Window(w) => w.wl_surface(),
            Self::Popup(p) => Some(Cow::Borrowed(p.wl_surface())),
            Self::Layer(l) => Some(Cow::Borrowed(l.wl_surface())),
            Self::Surface(s) => Some(Cow::Borrowed(s)),
        }
    }
}

impl WaylandFocus for PointerFocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Surface(s) => Some(Cow::Borrowed(s)),
            Self::X11(x) => x.wl_surface().map(Cow::Owned),
        }
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            Self::Surface(s) => s.same_client_as(object_id),
            Self::X11(x) => x.same_client_as(object_id),
        }
    }
}

impl KeyboardFocusTarget {
    fn inner(&self) -> &dyn KeyboardTarget<SpaceState> {
        match self {
            Self::Window(w) => match w.underlying_surface() {
                WindowSurface::Wayland(t) => t.wl_surface(),
                WindowSurface::X11(x) => x,
            },
            Self::Popup(p) => p.wl_surface(),
            Self::Layer(l) => l.wl_surface(),
            Self::Surface(s) => s,
        }
    }
}

impl PointerFocusTarget {
    fn inner(&self) -> &dyn PointerTarget<SpaceState> {
        match self {
            Self::Surface(s) => s,
            Self::X11(x) => x,
        }
    }
}

impl KeyboardTarget<SpaceState> for KeyboardFocusTarget {
    fn enter(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        self.inner().enter(seat, data, keys, serial)
    }

    fn leave(&self, seat: &Seat<SpaceState>, data: &mut SpaceState, serial: Serial) {
        self.inner().leave(seat, data, serial)
    }

    fn key(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: InputTime,
    ) {
        self.inner().key(seat, data, key, state, serial, time)
    }

    fn modifiers(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        self.inner().modifiers(seat, data, modifiers, serial)
    }
}

impl PointerTarget<SpaceState> for PointerFocusTarget {
    fn enter(&self, seat: &Seat<SpaceState>, data: &mut SpaceState, event: &MotionEvent) {
        self.inner().enter(seat, data, event)
    }
    fn motion(&self, seat: &Seat<SpaceState>, data: &mut SpaceState, event: &MotionEvent) {
        self.inner().motion(seat, data, event)
    }
    fn relative_motion(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        event: &RelativeMotionEvent,
    ) {
        self.inner().relative_motion(seat, data, event)
    }
    fn button(&self, seat: &Seat<SpaceState>, data: &mut SpaceState, event: &ButtonEvent) {
        self.inner().button(seat, data, event)
    }
    fn axis(&self, seat: &Seat<SpaceState>, data: &mut SpaceState, frame: AxisFrame) {
        self.inner().axis(seat, data, frame)
    }
    fn frame(&self, seat: &Seat<SpaceState>, data: &mut SpaceState) {
        self.inner().frame(seat, data)
    }
    fn leave(&self, seat: &Seat<SpaceState>, data: &mut SpaceState, serial: Serial, time: InputTime) {
        self.inner().leave(seat, data, serial, time)
    }
    fn gesture_swipe_begin(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        event: &GestureSwipeBeginEvent,
    ) {
        self.inner().gesture_swipe_begin(seat, data, event)
    }
    fn gesture_swipe_update(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        event: &GestureSwipeUpdateEvent,
    ) {
        self.inner().gesture_swipe_update(seat, data, event)
    }
    fn gesture_swipe_end(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        event: &GestureSwipeEndEvent,
    ) {
        self.inner().gesture_swipe_end(seat, data, event)
    }
    fn gesture_pinch_begin(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        event: &GesturePinchBeginEvent,
    ) {
        self.inner().gesture_pinch_begin(seat, data, event)
    }
    fn gesture_pinch_update(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        event: &GesturePinchUpdateEvent,
    ) {
        self.inner().gesture_pinch_update(seat, data, event)
    }
    fn gesture_pinch_end(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        event: &GesturePinchEndEvent,
    ) {
        self.inner().gesture_pinch_end(seat, data, event)
    }
    fn gesture_hold_begin(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        event: &GestureHoldBeginEvent,
    ) {
        self.inner().gesture_hold_begin(seat, data, event)
    }
    fn gesture_hold_end(
        &self,
        seat: &Seat<SpaceState>,
        data: &mut SpaceState,
        event: &GestureHoldEndEvent,
    ) {
        self.inner().gesture_hold_end(seat, data, event)
    }
}

/// Data a drag-and-drop offer carries while it hovers a target. Wayland and X11 clients
/// speak different DnD protocols (wl_data_offer vs XDND), so the offer type differs.
#[allow(dead_code)] // X11 offers arrive with XWayland (Stage 7.1).
pub enum SpaceOfferData<S: Source> {
    Wayland(WlOfferData<S>),
    X11(XwmOfferData<S>),
}

impl<S: Source> OfferData for SpaceOfferData<S> {
    fn disable(&self) {
        match self {
            Self::Wayland(d) => d.disable(),
            Self::X11(d) => d.disable(),
        }
    }
    fn drop(&self) {
        match self {
            Self::Wayland(d) => d.drop(),
            Self::X11(d) => d.drop(),
        }
    }
    fn validated(&self) -> bool {
        match self {
            Self::Wayland(d) => d.validated(),
            Self::X11(d) => d.validated(),
        }
    }
}

/// Drag-and-drop targets are pointer targets: the drop goes to the surface under the cursor.
impl DndFocus<SpaceState> for PointerFocusTarget {
    type OfferData<S>
        = SpaceOfferData<S>
    where
        S: Source;

    fn enter<S: Source>(
        &self,
        data: &mut SpaceState,
        dh: &DisplayHandle,
        source: Arc<S>,
        seat: &Seat<SpaceState>,
        location: Point<f64, Logical>,
        serial: &Serial,
    ) -> Option<SpaceOfferData<S>> {
        match self {
            Self::Surface(s) => DndFocus::enter(s, data, dh, source, seat, location, serial)
                .map(SpaceOfferData::Wayland),
            // XDND for X11 windows needs the X11 window manager (Stage 7.1).
            Self::X11(_) => None,
        }
    }

    fn motion<S: Source>(
        &self,
        data: &mut SpaceState,
        offer: Option<&mut SpaceOfferData<S>>,
        seat: &Seat<SpaceState>,
        location: Point<f64, Logical>,
        time: InputTime,
    ) {
        match (self, offer) {
            (Self::Surface(s), Some(SpaceOfferData::Wayland(o))) => {
                DndFocus::motion(s, data, Some(o), seat, location, time)
            }
            (Self::Surface(s), None) => {
                DndFocus::<SpaceState>::motion::<S>(s, data, None, seat, location, time)
            }
            // An offer of the other kind can't belong to this target; ignore it.
            _ => {}
        }
    }

    fn leave<S: Source>(
        &self,
        data: &mut SpaceState,
        offer: Option<&mut SpaceOfferData<S>>,
        seat: &Seat<SpaceState>,
    ) {
        match (self, offer) {
            (Self::Surface(s), Some(SpaceOfferData::Wayland(o))) => {
                DndFocus::leave(s, data, Some(o), seat)
            }
            (Self::Surface(s), None) => DndFocus::<SpaceState>::leave::<S>(s, data, None, seat),
            _ => {}
        }
    }

    fn drop<S: Source>(
        &self,
        data: &mut SpaceState,
        offer: Option<&mut SpaceOfferData<S>>,
        seat: &Seat<SpaceState>,
    ) {
        match (self, offer) {
            (Self::Surface(s), Some(SpaceOfferData::Wayland(o))) => {
                DndFocus::drop(s, data, Some(o), seat)
            }
            (Self::Surface(s), None) => DndFocus::<SpaceState>::drop::<S>(s, data, None, seat),
            _ => {}
        }
    }
}
