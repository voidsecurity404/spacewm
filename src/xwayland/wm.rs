//! Our X11 window manager: smithay's `X11Wm` asks, we decide.
//!
//! Normal X11 windows become panels exactly like xdg toplevels. Two X11 specifics:
//!
//! - X11 positions are global ("root" coordinates), and apps compute where their menus
//!   go from where they think their window is. So every X11 window gets a fixed spot in
//!   our 2D space and is configured to exactly that rectangle: space coordinates ARE root
//!   coordinates, and a menu's position minus its window's position is where it belongs
//!   on the panel.
//! - An X11 window's rectangle includes client-side shadows (GTK's _GTK_FRAME_EXTENTS).
//!   smithay's `X11Surface::geometry()` is the visible part *relative to that buffer*,
//!   like xdg geometry; the root position is `last_configure().loc`.
//! - Override-redirect windows (menus, tooltips, combo-box lists) bypass the window
//!   manager entirely: no map request, no decorations, they just appear at a root
//!   position and grab the pointer. They are not windows of their own to the user, so
//!   instead of making them panels we draw them into their parent's panel texture at
//!   that offset (like xdg popups), and route pointer input inside them to them.

use std::{cell::RefCell, os::fd::OwnedFd};

use glam::{Quat, Vec3};
use smithay::{
    desktop::Window,
    utils::{Logical, Point, Rectangle, Size, SERIAL_COUNTER},
    wayland::{
        selection::{
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
            SelectionTarget,
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        xwm::{Reorder, ResizeEdge, WmWindowProperty, XwmId},
        X11Surface, X11Wm, XwmHandler,
    },
};

use crate::{
    cli::RenderMode, focus::KeyboardFocusTarget, ipc::proto::Event, state::SpaceState,
    world::panel::PanelId,
};

/// An override-redirect window drawn inside a panel.
#[derive(Debug, Clone, PartialEq)]
pub struct X11Child {
    pub surface: X11Surface,
    /// Its buffer's top-left relative to the parent's buffer top-left (logical px): the
    /// coordinates of the parent's `bbox()`, in which xdg popups are positioned too.
    pub offset: Point<i32, Logical>,
}

impl X11Child {
    /// Its whole buffer (with its own shadows) in the parent's buffer coordinates.
    pub fn rect(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.offset, self.surface.bbox().size)
    }
}

/// Where the 2D space must place an X11 window so that its buffer lands exactly on its
/// root rectangle: the space positions a window's *geometry*, which sits
/// `geometry().loc` inside the buffer.
fn space_location(surface: &X11Surface, root: Point<i32, Logical>) -> Point<i32, Logical> {
    root + surface.geometry().loc
}

/// X window size (with shadows), panel position and rotation before fullscreen/maximize.
type Placement = (Size<i32, Logical>, Vec3, Quat);

/// Where a panel was before fullscreen/maximize made it big (stored on the X11Surface).
#[derive(Default)]
struct NormalPlacement(RefCell<Option<Placement>>);

impl XWaylandShellHandler for SpaceState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

impl XwmHandler for SpaceState {
    fn xwm_state(&mut self, xwm: XwmId) -> &mut X11Wm {
        // Callbacks only come from a wm's own event source, and we keep every wm (current
        // or retired) until that source's final `disconnected`, so the wm always exists.
        // There is no value we could return otherwise.
        let xw = self.xwayland.as_mut().expect("X11 callback without X11 support");
        match xw.wm.as_mut().filter(|w| w.id() == xwm) {
            Some(wm) => wm,
            None => {
                xw.retired.iter_mut().find(|w| w.id() == xwm).expect("X11 callback for unknown wm")
            }
        }
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    /// An X11 app wants to show a normal window: it becomes a panel.
    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(e) = surface.set_mapped(true) {
            tracing::warn!("cannot map X11 window: {e}");
            return;
        }
        // The size the app created its window with (shadows included).
        let requested = surface.last_configure().size;
        let size = if requested.w > 0 && requested.h > 0 {
            requested
        } else {
            crate::wayland::xdg_shell::DEFAULT_WINDOW_SIZE.into()
        };
        let position = self.next_cascade_position();
        if let Err(e) = surface.configure(Rectangle::new(position, size)) {
            tracing::warn!("cannot configure X11 window: {e}");
        }
        let window = Window::new_x11_window(surface.clone());
        self.space.map_element(window.clone(), space_location(&surface, position), false);
        let id = crate::state::window_id(&window);
        tracing::info!(id, class = surface.class(), title = surface.title(), "new X11 window");
        let visible = surface.geometry().size;
        self.add_panel(&window, (visible.w, visible.h), Some(surface.class()));
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let root = surface.last_configure();
        let window = Window::new_x11_window(surface.clone());
        // In the space too: the 2D renderer draws it, and commits find it there.
        self.space.map_element(window.clone(), space_location(&surface, root.loc), true);
        match self.x11_parent_panel(&surface) {
            Some((parent, parent_root)) => {
                if let Some(p) = self.world.panels_mut().iter_mut().find(|p| p.id == parent) {
                    p.x11_children.retain(|c| c.surface != surface);
                    let offset = root.loc - parent_root;
                    p.x11_children.push(X11Child { surface, offset });
                    p.content_dirty = true;
                }
                tracing::debug!(parent, ?root, "X11 override-redirect window");
            }
            None => {
                // Nothing to attach it to (a splash screen, a tray menu): its own panel.
                tracing::debug!(?root, "X11 override-redirect window without parent");
                let size = surface.geometry().size;
                self.add_panel(&window, (size.w, size.h), Some(surface.class()));
            }
        }
        self.queue_redraw();
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.forget_x11(&surface);
        if !surface.is_override_redirect() {
            let _ = surface.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.forget_x11(&surface);
    }

    /// Size requests are granted; position requests are ignored (positions live in 3D,
    /// and the 2D spot must stay fixed for menus to line up).
    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let mut geo = surface.last_configure();
        if let Some(w) = w {
            geo.size.w = (w as i32).clamp(1, 16384);
        }
        if let Some(h) = h {
            geo.size.h = (h as i32).clamp(1, 16384);
        }
        if let Err(e) = surface.configure(geo) {
            tracing::debug!("X11 configure failed: {e}");
        }
    }

    /// The X server says where a window (its buffer, in root coordinates) is now.
    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        if let Some(window) = self.x11_window(&surface) {
            self.space.map_element(window, space_location(&surface, geometry.loc), false);
        }
        // Children are stored relative to their parent: recompute when either moved.
        let root_of =
            |s: &X11Surface| if s == &surface { geometry.loc } else { s.last_configure().loc };
        for p in self.world.panels_mut() {
            let Some(parent) = p.window.x11_surface() else { continue };
            let parent_moved = parent == &surface;
            let parent_root = root_of(parent);
            for child in &mut p.x11_children {
                if parent_moved || child.surface == surface {
                    child.offset = root_of(&child.surface) - parent_root;
                    p.content_dirty = true;
                }
            }
        }
        self.queue_redraw();
    }

    /// A property changed. _GTK_FRAME_EXTENTS (shadow size) may arrive after mapping and
    /// moves the geometry inside the buffer: keep the 2D spot on the root rectangle.
    fn property_notify(&mut self, _xwm: XwmId, surface: X11Surface, _property: WmWindowProperty) {
        if let Some(window) = self.x11_window(&surface) {
            let loc = space_location(&surface, surface.last_configure().loc);
            if self.space.element_location(&window) != Some(loc) {
                self.space.map_element(window, loc, false);
                self.mark_x11_dirty(&surface);
            }
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let _ = surface.set_maximized(true);
        self.enlarge(&surface);
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let _ = surface.set_maximized(false);
        if !surface.is_fullscreen() {
            self.restore_normal(&surface);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if self.render_mode == RenderMode::Scene3D {
            // Same rule as for Wayland windows: only the window you are using.
            let panel = self.x11_panel(&surface);
            if let Some(id) = panel
                .filter(|&id| self.mode == (crate::input::mode::InputMode::Window { panel: id }))
            {
                self.enter_fullscreen(id);
            }
            return;
        }
        let _ = surface.set_fullscreen(true);
        self.enlarge(&surface);
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if self.render_mode == RenderMode::Scene3D {
            if self.x11_panel(&surface).is_some_and(|id| self.fullscreen_panel() == Some(id)) {
                self.exit_fullscreen();
            }
            return;
        }
        let _ = surface.set_fullscreen(false);
        if !surface.is_maximized() {
            self.restore_normal(&surface);
        }
    }

    fn resize_request(&mut self, _xwm: XwmId, surface: X11Surface, _b: u32, edge: ResizeEdge) {
        tracing::debug!(class = surface.class(), ?edge, "X11 resize request ignored");
    }

    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, _button: u32) {
        // Moving is grabbing the panel in 3D (Mod+G).
        tracing::debug!(class = surface.class(), "X11 move request ignored");
    }

    /// X11 apps may read the Wayland clipboard only while one of them has focus, the
    /// same rule as for Wayland clients (seat.rs).
    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        let Some(keyboard) = self.seat.get_keyboard() else { return false };
        matches!(
            keyboard.current_focus(),
            Some(KeyboardFocusTarget::Window(w))
                if w.x11_surface().and_then(|s| s.xwm_id()) == Some(xwm)
        )
    }

    /// An X11 app pastes: pipe the Wayland selection into its fd.
    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let result = match selection {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&self.seat, mime_type, fd)
                    .map_err(|e| e.to_string())
            }
            SelectionTarget::Primary => request_primary_client_selection(&self.seat, mime_type, fd)
                .map_err(|e| e.to_string()),
        };
        if let Err(e) = result {
            tracing::debug!(?selection, "Wayland selection for X11 unavailable: {e}");
        }
    }

    /// An X11 app copied: offer it to Wayland apps (the data is fetched on paste, see
    /// SelectionHandler::send_selection in data_device.rs).
    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display_handle, &self.seat, mime_types, ())
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, ())
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display_handle, &self.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display_handle, &self.seat)
                }
            }
        }
    }

    /// The X11 connection closed: the last callback of this wm. Either a server that
    /// failed to start finished exiting (drop its retired wm), or the current one died.
    fn disconnected(&mut self, xwm: XwmId) {
        let Some(xw) = self.xwayland.as_mut() else { return };
        xw.retired.retain(|w| w.id() != xwm);
        if xw.wm.as_ref().is_some_and(|w| w.id() == xwm) {
            tracing::warn!("Xwayland connection lost");
            super::server_gone(self);
            if let Some(xw) = self.xwayland.as_mut() {
                xw.retired.retain(|w| w.id() != xwm);
            }
        }
    }
}

impl SpaceState {
    /// Put a new window into the world and announce it; shared by X11 windows and
    /// override-redirect windows without a parent.
    fn add_panel(&mut self, window: &Window, size: (i32, i32), app_id: Option<String>) {
        let id = crate::state::window_id(window);
        let distance = self.spawn_distance(size);
        self.world.add_window(id, window.clone(), size, &self.view.camera, distance);
        // X11 apps have their class from the start: apply a saved spot right away.
        self.restore_placement(window);
        self.view.dirty = true;
        crate::ipc::broadcast(self, Event::PanelOpened { id, app_id });
        self.launch_window_opened(window);
        if self.render_mode == RenderMode::Scene3D {
            crate::input::mode::on_new_window(self, id);
        } else {
            self.focus_window(Some(window), SERIAL_COUNTER.next_serial());
        }
        self.queue_redraw();
    }

    fn mark_x11_dirty(&mut self, surface: &X11Surface) {
        if let Some(p) =
            self.world.panels_mut().iter_mut().find(|p| p.window.x11_surface() == Some(surface))
        {
            p.content_dirty = true;
        }
        self.queue_redraw();
    }

    /// The panel showing an X11 window.
    fn x11_panel(&self, surface: &X11Surface) -> Option<crate::world::panel::PanelId> {
        self.world.panels().iter().find(|p| p.window.x11_surface() == Some(surface)).map(|p| p.id)
    }

    pub fn x11_window(&self, surface: &X11Surface) -> Option<Window> {
        self.space.elements().find(|w| w.x11_surface() == Some(surface)).cloned()
    }

    /// Which X11 panel an override-redirect window belongs to, and that window's root
    /// position: the window it declares itself transient for, else the focused X11 window
    /// (menus open from what you are using), else an X11 window whose rectangle holds it.
    fn x11_parent_panel(&self, surface: &X11Surface) -> Option<(PanelId, Point<i32, Logical>)> {
        let x11_panels = || {
            self.world.panels().iter().filter_map(|p| Some((p.id, p.window.x11_surface()?.clone())))
        };
        let (id, parent) = surface
            .is_transient_for()
            .and_then(|parent| x11_panels().find(|(_, s)| s.window_id() == parent))
            .or_else(|| {
                let focused = self.seat.get_keyboard().and_then(|k| k.current_focus());
                let Some(KeyboardFocusTarget::Window(w)) = focused else { return None };
                let s = w.x11_surface().filter(|s| s.xwm_id() == surface.xwm_id())?.clone();
                Some((self.world.panel_for_window(&w)?.id, s))
            })
            .or_else(|| {
                let loc = surface.last_configure().loc;
                x11_panels().find(|(_, s)| s.last_configure().contains(loc))
            })?;
        Some((id, parent.last_configure().loc))
    }

    /// An X11 window was unmapped or destroyed: drop its panel or its place in a parent.
    fn forget_x11(&mut self, surface: &X11Surface) {
        for p in self.world.panels_mut() {
            let before = p.x11_children.len();
            p.x11_children.retain(|c| &c.surface != surface);
            if p.x11_children.len() != before {
                p.content_dirty = true;
            }
        }
        if let Some(window) = self.x11_window(surface) {
            if self.world.panel_for_window(&window).is_some() {
                self.remove_window(&window);
            } else {
                self.space.unmap_elem(&window);
            }
        }
        self.queue_redraw();
    }

    /// Fullscreen/maximize in 3D: the window gets the output's size and its panel moves
    /// in front of the camera, big enough to fill most of the view.
    fn enlarge(&mut self, surface: &X11Surface) {
        let Some(window) = self.x11_window(surface) else { return };
        let Some(output) = self.primary_output_geometry() else { return };
        let root = surface.last_configure();
        let size = output.size;
        let distance = self.spawn_distance((size.w, size.h));
        let camera = self.view.camera;
        if let Some(p) = self.world.panels_mut().iter_mut().find(|p| p.window == window) {
            surface.user_data().insert_if_missing(NormalPlacement::default);
            if let Some(saved) = surface.user_data().get::<NormalPlacement>() {
                let mut saved = saved.0.borrow_mut();
                if saved.is_none() {
                    *saved = Some((root.size, p.position, p.rotation));
                }
            }
            p.position = camera.position + camera.forward() * distance;
            p.face_towards(camera.position);
        }
        if let Err(e) = surface.configure(Rectangle::new(root.loc, size)) {
            tracing::debug!("X11 configure failed: {e}");
        }
        self.view.dirty = true;
        self.queue_redraw();
    }

    fn restore_normal(&mut self, surface: &X11Surface) {
        let Some(window) = self.x11_window(surface) else { return };
        let saved =
            surface.user_data().get::<NormalPlacement>().and_then(|s| s.0.borrow_mut().take());
        let Some((size, position, rotation)) = saved else { return };
        if let Some(p) = self.world.panels_mut().iter_mut().find(|p| p.window == window) {
            p.position = position;
            p.rotation = rotation;
        }
        if let Err(e) = surface.configure(Rectangle::new(surface.last_configure().loc, size)) {
            tracing::debug!("X11 configure failed: {e}");
        }
        self.view.dirty = true;
        self.queue_redraw();
    }
}
