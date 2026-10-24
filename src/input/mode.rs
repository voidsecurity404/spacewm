//! Input modes in 3D.
//!
//! - Walk: the mouse turns the camera, WASD moves; NOTHING reaches clients. A crosshair
//!   marks the screen center; the panel under it is highlighted.
//! - Window: one panel's client gets the keyboard, and the mouse moves a cursor across
//!   that panel as if it were a normal monitor. The camera stays still — turning the view
//!   while you try to click a button would make every click miss.
//!
//! Mod+Escape always returns to Walk (see `route_key`).

use std::time::{Duration, Instant};

use smithay::{
    backend::input::InputTime,
    desktop::{Window, WindowSurfaceType},
    input::pointer::MotionEvent,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use super::raycast::{self, PanelHit};
use crate::{
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    ipc::proto::Event,
    state::SpaceState,
    world::panel::PanelId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Camera control; nothing goes to clients.
    #[default]
    Walk,
    /// Keyboard and pointer go to this panel's client.
    Window { panel: PanelId },
}

impl InputMode {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Walk => "walk",
            Self::Window { .. } => "window",
        }
    }
}

/// Which panels get a border this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Highlight {
    /// Walk mode: the panel under the crosshair, within reach, seen from the front.
    pub hovered: Option<PanelId>,
    /// Window mode: the panel being used.
    pub focused: Option<PanelId>,
}

/// Input tunables, from the `[input]` config section.
#[derive(Debug, Clone, Copy)]
pub struct InputSettings {
    /// Radians per pixel of mouse motion.
    pub mouse_sensitivity: f32,
    /// Mouse forward looks down.
    pub invert_y: bool,
    /// Metres per second.
    pub walk_speed: f32,
    /// Panels farther than this can't be entered by clicking.
    pub reach_distance: f32,
    /// 1.0 = the cursor on a panel moves as fast as a normal desktop cursor.
    pub window_cursor_speed: f64,
    /// Touchpads: tap to click, natural (content follows fingers) scrolling.
    pub touchpad_tap: bool,
    pub touchpad_natural_scroll: bool,
    /// Mice: no pointer acceleration (consistent aiming in Walk mode).
    pub mouse_accel_flat: bool,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 0.0025,
            invert_y: false,
            walk_speed: crate::world::player::WALK_SPEED,
            reach_distance: 6.0,
            window_cursor_speed: 1.0,
            touchpad_tap: true,
            touchpad_natural_scroll: true,
            mouse_accel_flat: true,
        }
    }
}

impl InputSettings {
    pub fn from_config(c: &crate::config::Input) -> Self {
        Self {
            mouse_sensitivity: c.mouse_sensitivity,
            invert_y: c.invert_y,
            walk_speed: c.walk_speed,
            reach_distance: c.reach_distance,
            window_cursor_speed: c.window_cursor_speed,
            touchpad_tap: c.touchpad_tap,
            touchpad_natural_scroll: c.touchpad_natural_scroll,
            mouse_accel_flat: c.mouse_accel == crate::config::MouseAccel::Flat,
        }
    }
}

/// A window opening this soon after the user started a program gets focus; any other new
/// window must be clicked (no focus stealing by background apps).
pub const SPAWN_FOCUS_WINDOW: Duration = Duration::from_secs(5);

/// THE place where modes change.
pub fn set_mode(state: &mut SpaceState, new_mode: InputMode, hit: Option<&PanelHit>) {
    let old = state.mode;
    if old == new_mode {
        return;
    }
    tracing::debug!(from = ?old, to = ?new_mode, "input mode");
    let serial = SERIAL_COUNTER.next_serial();
    // Fullscreen belongs to one window: leaving it (Mod+Escape, another window) ends it.
    if state.fullscreen.is_some_and(|f| new_mode != (InputMode::Window { panel: f.panel })) {
        state.exit_fullscreen();
    }
    if new_mode != InputMode::Walk {
        state.close_inventory();
        state.set_dock_cursor(false);
    }

    // Deactivate the window we are leaving (clients draw unfocused decorations).
    if let InputMode::Window { panel } = old {
        if let Some(window) = panel_window(state, panel) {
            set_activated(&window, false);
        }
    }

    match new_mode {
        InputMode::Window { panel } => {
            let Some(window) = panel_window(state, panel) else {
                return;
            };
            state.mode = new_mode;
            // Where the cursor starts: where the user clicked, else the window center. A
            // window that was just opened has no buffer yet (size 0): use the size the
            // panel was made with, or the cursor would start in the corner.
            let geo = window.geometry().size;
            let (w, h) = if geo.w > 0 && geo.h > 0 {
                (geo.w, geo.h)
            } else {
                state.world.panels().iter().find(|p| p.id == panel).map_or((0, 0), |p| p.size_px)
            };
            state.virtual_cursor = match hit {
                Some(h) if h.panel == panel => h.surface_px,
                _ => Point::from((w as f64 / 2.0, h as f64 / 2.0)),
            };
            set_activated(&window, true);
            state.set_keyboard_focus(Some(KeyboardFocusTarget::Window(window)));
            send_window_pointer(state, InputTime::now());
            state.focus_view_enter(panel);
        }
        InputMode::Walk => {
            state.mode = new_mode;
            state.set_keyboard_focus(None);
            state.focus_view_leave();
            // Pointer leaves every surface.
            if let Some(pointer) = state.seat.get_pointer() {
                let location = pointer.current_location();
                pointer.motion(
                    state,
                    None,
                    &MotionEvent { location, serial, time: InputTime::now() },
                );
                pointer.frame(state);
            }
        }
    }
    // Keys held for walking must not keep moving us once we're in a window.
    state.view.controls.clear();
    state.view.dirty = true;
    state.queue_redraw();
    crate::ipc::broadcast(state, Event::ModeChanged { mode: new_mode.name().into() });
}

fn panel_window(state: &SpaceState, panel: PanelId) -> Option<Window> {
    state.world.panels().iter().find(|p| p.id == panel).map(|p| p.window.clone())
}

fn set_activated(window: &Window, activated: bool) {
    window.set_activated(activated);
    if let Some(toplevel) = window.toplevel() {
        toplevel.send_pending_configure();
    }
}

/// Recompute the panel under the crosshair and the highlight. Called before each frame.
pub fn update_hover(state: &mut SpaceState) {
    let ray = raycast::camera_center_ray(&state.view.camera);
    let reach = state.input_settings.reach_distance;
    // Blocks occlude panels: a panel behind a wall can't be hovered or clicked.
    let block = raycast::voxel_ray(&ray, 128.0, |c| state.voxels.solid(c));
    let panel = raycast::pick(&ray, &state.world, 1000.0);
    state.hover = panel.filter(|p| block.is_none_or(|b| p.t < b.t));
    let target = block
        .filter(|b| b.t <= reach && state.mode == InputMode::Walk && state.grab.is_none())
        .filter(|b| state.hover.is_none_or(|p| b.t < p.t));
    if target.map(|t| t.block) != state.target_block.map(|t| t.block) {
        state.view.dirty = true; // the outline moved
    }
    state.target_block = target;
    let highlight = match state.mode {
        InputMode::Walk => Highlight {
            hovered: state.hover.filter(|h| !h.from_back && h.t <= reach).map(|h| h.panel),
            focused: None,
        },
        InputMode::Window { panel } => Highlight { hovered: None, focused: Some(panel) },
    };
    if highlight != state.highlight {
        state.highlight = highlight;
        state.view.dirty = true;
    }
}

/// Walk mode, left click: enter the panel under the crosshair, if reachable.
/// Returns true if a panel was entered.
pub fn try_enter_hovered(state: &mut SpaceState) -> bool {
    update_hover(state);
    let reach = state.input_settings.reach_distance;
    match state.hover {
        Some(hit) if !hit.from_back && hit.t <= reach => {
            set_mode(state, InputMode::Window { panel: hit.panel }, Some(&hit));
            true
        }
        _ => false,
    }
}

/// A new toplevel appeared. In 3D it gets focus only if the user just asked for a new
/// program; otherwise it waits to be clicked.
pub fn on_new_window(state: &mut SpaceState, panel: PanelId) {
    let expected = state.expect_window_until.is_some_and(|until| Instant::now() < until);
    if expected {
        state.expect_window_until = None;
        set_mode(state, InputMode::Window { panel }, None);
    }
}

/// The focused panel went away: back to walking.
pub fn on_panel_removed(state: &mut SpaceState, panel: PanelId) {
    state.ws_remove_window(panel);
    if state.drag.is_some_and(|d| d.panel == panel) {
        state.drag = None;
    }
    if state.fullscreen_panel() == Some(panel) {
        state.exit_fullscreen();
    }
    if state.mode == (InputMode::Window { panel }) {
        set_mode(state, InputMode::Walk, None);
    }
    // Seated: close the gap on the arc.
    state.arrange_seated();
}

/// Mod+Tab: turn the view to the next window (by id, wrapping around) and enter it when
/// it is within reach and seen from the front; otherwise just look at it, so a window
/// lost somewhere in the world is easy to find.
pub fn next_window(state: &mut SpaceState) {
    if state.render_mode != crate::cli::RenderMode::Scene3D || state.lock.is_locked() {
        return;
    }
    let current = match state.mode {
        InputMode::Window { panel } => Some(panel),
        InputMode::Walk => state.highlight.hovered,
    };
    let mut ids: Vec<PanelId> =
        state.world.panels().iter().filter(|p| p.visible).map(|p| p.id).collect();
    ids.sort_unstable();
    let Some(&next) = current.and_then(|c| ids.iter().find(|&&id| id > c)).or_else(|| ids.first())
    else {
        return;
    };
    let Some(p) = state.world.panels().iter().find(|p| p.id == next) else { return };
    let (target, normal) = (p.position, p.normal());
    if state.fullscreen.is_some() {
        state.exit_fullscreen();
    }
    set_mode(state, InputMode::Walk, None);
    // We set the view ourselves: no glide back to the old spot.
    state.focus_view_cancel();
    let camera = &mut state.view.camera;
    let d = target - camera.position;
    // Yaw 0 looks along -Z (see Camera::forward); pitch up is positive.
    camera.yaw = (d.x).atan2(-d.z).rem_euclid(std::f32::consts::TAU);
    camera.pitch = d.y.atan2(d.x.hypot(d.z)).clamp(-1.5, 1.5);
    let in_reach = d.length() <= state.input_settings.reach_distance && normal.dot(-d) > 0.0;
    tracing::debug!(panel = next, in_reach, "next window");
    state.view.dirty = true;
    state.queue_redraw();
    if in_reach {
        set_mode(state, InputMode::Window { panel: next }, None);
    }
}

/// Window mode: relative mouse motion moves the cursor across the panel. The speed is
/// matched to how big the panel looks: moving the mouse 100 px moves the cursor by what
/// looks like 100 px on screen, whatever the distance.
pub fn move_virtual_cursor(state: &mut SpaceState, delta: Point<f64, Logical>, time: InputTime) {
    let InputMode::Window { panel } = state.mode else {
        return;
    };
    // A game locked the pointer (mouse-look): the cursor stays, only raw deltas go on.
    if state.pointer_locked() {
        tracing::debug!("pointer locked by the app: cursor stays");
        return;
    }
    let Some(p) = state.world.panels().iter().find(|p| p.id == panel) else {
        return;
    };
    let factor = if state.fullscreen_panel() == Some(panel) {
        // Fullscreen: one window pixel per screen pixel.
        state.input_settings.window_cursor_speed
    } else {
        let output_h = state.primary_output_geometry().map(|g| g.size.h).unwrap_or(1080) as f64;
        let camera = &state.view.camera;
        let distance = camera.position.distance(p.position).max(0.05) as f64;
        // Screen px per metre at that distance, and panel px per metre.
        let screen_ppm =
            output_h / (2.0 * distance * (camera.fov_y_deg as f64 / 2.0).to_radians().tan());
        p.pixels_per_unit as f64 / screen_ppm * state.input_settings.window_cursor_speed
    };
    let bbox = p.bbox_px;
    let mut c = state.virtual_cursor + delta.upscale(factor);
    // Clamp to the window's bbox (not just its geometry) so popups that hang outside the
    // window stay reachable.
    c.x = c.x.clamp(bbox.loc.x as f64, (bbox.loc.x + bbox.size.w) as f64 - 1.0);
    c.y = c.y.clamp(bbox.loc.y as f64, (bbox.loc.y + bbox.size.h) as f64 - 1.0);
    state.virtual_cursor = c;
    send_window_pointer(state, time);
    state.view.dirty = true;
    state.queue_redraw();
}

/// Window mode, absolute pointer (tablets, tests): put the cursor where the screen point
/// lands on the focused panel's plane.
pub fn point_virtual_cursor(state: &mut SpaceState, screen: Point<f64, Logical>, time: InputTime) {
    let InputMode::Window { panel } = state.mode else {
        return;
    };
    let Some(geo) = state.primary_output_geometry() else {
        return;
    };
    let Some(p) = state.world.panels().iter().find(|p| p.id == panel) else {
        return;
    };
    if state.fullscreen_panel() == Some(panel) {
        // The window covers the output with its geometry at the top-left corner.
        let bbox = p.bbox_px;
        let local = screen - geo.loc.to_f64();
        state.virtual_cursor = Point::from((
            local.x.clamp(bbox.loc.x as f64, (bbox.loc.x + bbox.size.w) as f64 - 1.0),
            local.y.clamp(bbox.loc.y as f64, (bbox.loc.y + bbox.size.h) as f64 - 1.0),
        ));
        send_window_pointer(state, time);
        state.queue_redraw();
        return;
    }
    let ray = raycast::camera_screen_ray(
        &state.view.camera,
        screen - geo.loc.to_f64(),
        (geo.size.w, geo.size.h),
    );
    // Intersect the plane even outside the rectangle, then clamp like relative motion.
    let Some(t) = raycast::ray_plane(&ray, p.position, p.normal()) else {
        return;
    };
    let local = p.model_matrix().inverse().transform_point3(ray.origin + ray.dir * t);
    let (w, h) = (p.size_px.0 as f64, p.size_px.1 as f64);
    let bbox = p.bbox_px;
    let x = ((local.x as f64 + 0.5) * w)
        .clamp(bbox.loc.x as f64, (bbox.loc.x + bbox.size.w) as f64 - 1.0);
    let y = ((0.5 - local.y as f64) * h)
        .clamp(bbox.loc.y as f64, (bbox.loc.y + bbox.size.h) as f64 - 1.0);
    state.virtual_cursor = Point::from((x, y));
    send_window_pointer(state, time);
    state.view.dirty = true;
    state.queue_redraw();
}

/// Tell the client where the cursor is on its window. The pointer's "global" coordinate
/// space in Window mode is the window's geometry space (the virtual cursor).
pub fn send_window_pointer(state: &mut SpaceState, time: InputTime) {
    let InputMode::Window { panel } = state.mode else {
        return;
    };
    let Some(p) = state.world.panels().iter().find(|p| p.id == panel) else {
        return;
    };
    let under = window_surface_under(&p.window, &p.x11_children, state.virtual_cursor);
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let location = state.virtual_cursor;
    pointer.motion(
        state,
        under,
        &MotionEvent { location, serial: SERIAL_COUNTER.next_serial(), time },
    );
    pointer.frame(state);
    state.activate_pointer_constraint();
}

/// The surface (toplevel, subsurface, popup or X11 menu) at `point` in window-geometry
/// coordinates, and that surface's origin in the same coordinates.
pub fn window_surface_under(
    window: &Window,
    x11_children: &[crate::xwayland::X11Child],
    point: Point<f64, Logical>,
) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
    let geo_loc = window.geometry().loc.to_f64();
    // X11 menus are on top of their window; the newest one is front-most. Their rects are
    // in buffer coordinates, `point` is in geometry coordinates.
    for child in x11_children.iter().rev() {
        if child.rect().to_f64().contains(point + geo_loc) {
            let origin = child.offset.to_f64() - geo_loc;
            return Some((PointerFocusTarget::X11(child.surface.clone()), origin));
        }
    }
    // surface_under works relative to the buffer origin, which is geo.loc before geometry.
    let (surface, surface_loc) = window.surface_under(point + geo_loc, WindowSurfaceType::ALL)?;
    let target = match window.x11_surface() {
        Some(x11) if x11.wl_surface().as_ref() == Some(&surface) => {
            PointerFocusTarget::X11(x11.clone())
        }
        _ => PointerFocusTarget::Surface(surface),
    };
    Some((target, surface_loc.to_f64() - geo_loc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_walk() {
        assert_eq!(InputMode::default(), InputMode::Walk);
        assert_eq!(InputMode::Window { panel: 3 }.name(), "window");
    }
}
