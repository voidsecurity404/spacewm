//! xdg_shell: application windows and popups.
//!
//! A bare wl_surface has no meaning. xdg_shell gives it a role: an `xdg_toplevel` is an
//! application window (title, app_id, size negotiation), an `xdg_popup` is a menu or
//! tooltip positioned relative to its parent.
//!
//! Size negotiation works through configure/ack: we send a configure with the size and
//! state (activated, maximized, ...), the client acks it and commits a buffer of that size.

use smithay::wayland::seat::WaylandFocus;
use smithay::{
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, PopupKeyboardGrab, PopupKind,
        PopupPointerGrab, PopupUngrabStrategy, Window,
    },
    input::{pointer::Focus, Seat},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::{wl_output::WlOutput, wl_seat, wl_surface::WlSurface},
    },
    utils::{Logical, Point, Rectangle, Serial, SERIAL_COUNTER},
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};

use crate::{cli::RenderMode, focus::KeyboardFocusTarget, ipc::proto::Event, state::SpaceState};

/// 3D mode: popups may extend this far (px) beyond their window before being moved back.
const POPUP_MARGIN_3D: i32 = 400;

/// Size every new window is offered before its first configure.
pub const DEFAULT_WINDOW_SIZE: (i32, i32) = (1000, 700);
/// 2D cascade: each new window is offset this much from the previous one.
const CASCADE_STEP: i32 = 40;
const CASCADE_WRAP: i32 = 10;

impl XdgShellHandler for SpaceState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.size = Some(DEFAULT_WINDOW_SIZE.into());
        });
        let window = Window::new_wayland_window(surface);
        let position = self.next_cascade_position();
        self.space.map_element(window.clone(), position, false);
        let id = crate::state::window_id(&window);
        tracing::info!(id, x = position.x, y = position.y, "new toplevel");
        // In 3D the window becomes a panel in front of the camera.
        let distance = self.spawn_distance(DEFAULT_WINDOW_SIZE);
        self.world.add_window(id, window.clone(), DEFAULT_WINDOW_SIZE, &self.view.camera, distance);
        self.view.dirty = true;
        crate::ipc::broadcast(self, Event::PanelOpened { id, app_id: None });

        if self.render_mode == RenderMode::Scene3D {
            // 3D: only a window the user just asked for takes focus (no focus stealing).
            crate::input::mode::on_new_window(self, id);
        } else {
            // 2D: new windows get keyboard focus, like on any desktop.
            self.focus_window(Some(&window), SERIAL_COUNTER.next_serial());
        }
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        // The menu disappears from its parent's panel texture.
        if let Some(parent) = surface.get_parent_surface() {
            self.mark_surface_dirty(&parent);
        }
        self.queue_redraw();
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!("failed to track popup: {err}");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    /// A popup asked for an explicit grab: clicks outside it must close it, and keyboard
    /// input goes to it (menus with arrow-key navigation).
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        if self.lock.is_locked() {
            return; // no menu grabs the input away from the lock screen
        }
        let Some(seat) = Seat::<SpaceState>::from_resource(&seat) else {
            return;
        };
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let Some(window) = self.window_for_surface(&root) else {
            return;
        };
        let Ok(mut grab) =
            self.popups.grab_popup(KeyboardFocusTarget::Window(window), kind, &seat, serial)
        else {
            return;
        };

        // A grab is only valid in response to a recent user action (the click that opened
        // the menu). If another grab is active that this one doesn't continue, refuse.
        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // Interactive move is replaced by grabbing panels in 3D (Stage 5).
        tracing::debug!(app_id = ?toplevel_app_id(surface.wl_surface()), "move request ignored");
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        tracing::debug!(
            app_id = ?toplevel_app_id(surface.wl_surface()),
            ?edges,
            "resize request ignored"
        );
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        // Maximized windows leave the strips that bars reserved (exclusive zones) free.
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(full) = self.space.output_geometry(&output) else {
            return;
        };
        let zone = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
        let output_geo = smithay::utils::Rectangle::new(full.loc + zone.loc, zone.size);
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(output_geo.size);
            });
            self.space.map_element(window, output_geo.loc, true);
        }
        surface.send_configure();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = Some(DEFAULT_WINDOW_SIZE.into());
        });
        surface.send_pending_configure();
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        let Some(output_geo) = self.primary_output_geometry() else {
            return;
        };
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Fullscreen);
                state.size = Some(output_geo.size);
            });
            self.space.map_element(window, output_geo.loc, true);
        }
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = Some(DEFAULT_WINDOW_SIZE.into());
        });
        surface.send_pending_configure();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        tracing::info!(app_id = ?toplevel_app_id(surface.wl_surface()), "toplevel destroyed");
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.remove_window(&window);
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        tracing::info!(app_id = ?toplevel_app_id(surface.wl_surface()), "app_id set");
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.restore_placement(&window);
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        tracing::debug!(title = ?toplevel_title(surface.wl_surface()), "title changed");
    }
}

impl SpaceState {
    /// A window (Wayland or X11) is gone: drop its panel, remember where it was, and fix
    /// up focus, mode and grabs that pointed at it.
    pub fn remove_window(&mut self, window: &Window) {
        self.space.unmap_elem(window);
        // Remember where it was, so it comes back to the same spot next time.
        if let Some(saved) = self.world.panel_for_window(window).and_then(crate::state::saved_panel)
        {
            self.layout.remember(saved);
            self.layout_changed();
        }
        if self
            .grab
            .is_some_and(|g| self.world.panel_for_window(window).is_some_and(|p| p.id == g.panel))
        {
            self.grab = None;
        }
        self.world.remove_window(window);
        self.view.dirty = true;
        let id = crate::state::window_id(window);
        crate::input::mode::on_panel_removed(self, id);
        crate::ipc::broadcast(self, Event::PanelClosed { id });

        // If the focused window went away, focus the top-most remaining one (2D).
        let keyboard_lost = self
            .seat
            .get_keyboard()
            .map(|k| match k.current_focus() {
                Some(KeyboardFocusTarget::Window(w)) => &w == window,
                None => true,
                _ => false,
            })
            .unwrap_or(false);
        if keyboard_lost && self.render_mode == RenderMode::Flat2D {
            let top = self
                .space
                .elements()
                .rev()
                .find(|w| !w.x11_surface().is_some_and(|x| x.is_override_redirect()))
                .cloned();
            self.focus_window(top.as_ref(), SERIAL_COUNTER.next_serial());
        }
        self.queue_redraw();
    }

    pub fn next_cascade_position(&mut self) -> Point<i32, Logical> {
        let origin = self.primary_output_geometry().map(|g| g.loc).unwrap_or_default();
        let i = self.cascade_index;
        self.cascade_index = (self.cascade_index + 1) % CASCADE_WRAP;
        origin + Point::from((CASCADE_STEP * i + 20, CASCADE_STEP * i + 20))
    }

    /// Give keyboard focus to `window` (or nobody), mark it activated and raise it.
    pub fn focus_window(&mut self, window: Option<&Window>, serial: Serial) {
        if let Some(window) = window {
            self.space.raise_element(window, true);
        } else {
            for w in self.space.elements() {
                w.set_activated(false);
            }
        }
        // `activated` is part of the pending configure; clients draw focused decorations.
        for w in self.space.elements() {
            if let Some(toplevel) = w.toplevel() {
                toplevel.send_pending_configure();
            }
        }
        let _ = serial;
        self.set_keyboard_focus(window.cloned().map(KeyboardFocusTarget::Window));
    }

    /// Keep popups on screen: the positioner says where the client wants the popup;
    /// we flip/slide it so it stays inside the output.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.window_for_surface(&root) else {
            return;
        };
        let toplevel_coords = get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        // The target is the area the popup must stay inside, relative to the popup's parent.
        let mut target = if self.render_mode == RenderMode::Scene3D {
            // In 3D there is no screen edge to respect: the popup is drawn into its
            // window's panel, which grows to include it. Keep it near the window (a
            // generous margin around the window geometry) so menus don't flip for no reason.
            let geo = window.geometry();
            Rectangle::new(
                (-POPUP_MARGIN_3D, -POPUP_MARGIN_3D).into(),
                (geo.size.w + 2 * POPUP_MARGIN_3D, geo.size.h + 2 * POPUP_MARGIN_3D).into(),
            )
        } else {
            let Some(output_geo) = self.primary_output_geometry() else {
                return;
            };
            let Some(window_geo) = self.space.element_geometry(&window) else {
                return;
            };
            let mut t = output_geo;
            t.loc -= window_geo.loc;
            t
        };
        target.loc -= toplevel_coords;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    pub fn primary_output_geometry(&self) -> Option<smithay::utils::Rectangle<i32, Logical>> {
        let output = self.space.outputs().next()?;
        self.space.output_geometry(output)
    }
}

/// Called from the compositor commit handler. xdg_shell requires the compositor to send
/// an initial configure before the client may attach a buffer; the client signals it is
/// ready by committing without a buffer. Same for popups.
pub fn handle_commit(state: &mut SpaceState, surface: &WlSurface) {
    if let Some(window) =
        state.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned()
    {
        if let Some(toplevel) = window.toplevel() {
            if !toplevel.is_initial_configure_sent() {
                toplevel.send_configure();
            }
        }
    }

    state.popups.commit(surface);
    if let Some(PopupKind::Xdg(popup)) = state.popups.find_popup(surface) {
        if !popup.is_initial_configure_sent() {
            // The initial configure is always allowed, so this cannot fail.
            if let Err(err) = popup.send_configure() {
                tracing::warn!("initial popup configure failed: {err}");
            }
        }
    }
}

fn with_toplevel_data<T>(
    surface: &WlSurface,
    f: impl FnOnce(&smithay::wayland::shell::xdg::XdgToplevelSurfaceRoleAttributes) -> T,
) -> Option<T> {
    with_states(surface, |states| {
        let data = states.data_map.get::<XdgToplevelSurfaceData>()?;
        let data = data.lock().ok()?;
        Some(f(&data))
    })
}

pub fn toplevel_app_id(surface: &WlSurface) -> Option<String> {
    with_toplevel_data(surface, |d| d.app_id.clone()).flatten()
}

pub fn toplevel_title(surface: &WlSurface) -> Option<String> {
    with_toplevel_data(surface, |d| d.title.clone()).flatten()
}
