//! Doing things in the world from Walk mode: carrying panels around, hanging them on
//! walls, rotating/scaling/hiding them, and breaking/placing blocks.

use glam::{Quat, Vec3};
use smithay::input::keyboard::ModifiersState;

use super::{
    mode::{set_mode, InputMode},
    raycast::{self, VoxelHit},
};
use crate::{
    state::SpaceState,
    world::{
        block::Block,
        panel::{orientation, PanelId},
    },
};

/// A panel being carried.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grab {
    pub panel: PanelId,
    /// How far in front of the eyes it floats (scroll wheel changes it).
    pub distance: f32,
    /// Extra rotation around Y on top of "facing the camera" (Mod+R).
    pub yaw_offset: f32,
    /// Extra tilt on top of the automatic one (Mod+PageUp / Mod+PageDown).
    pub tilt_offset: f32,
}

const GRAB_MIN: f32 = 0.8;
const GRAB_MAX: f32 = 8.0;
/// A panel hung on a wall floats this far in front of it (no z-fighting with the wall).
const WALL_GAP: f32 = 0.02;
const PPU_MIN: f32 = 200.0;
/// One Mod+PageUp / Mod+PageDown press tilts a panel this much.
pub const TILT_STEP: f32 = 15.0 * std::f32::consts::PI / 180.0;
/// Panels never tilt further than this (past it the window is seen edge-on).
pub const TILT_MAX: f32 = 75.0 * std::f32::consts::PI / 180.0;
/// Carried panels at about eye height stay upright: the automatic tilt starts past this
/// angle, so a small look up or down doesn't leave every panel slightly crooked.
const AUTO_TILT_DEAD: f32 = 8.0 * std::f32::consts::PI / 180.0;

/// The automatic tilt for a carried panel at `pos`: it leans to face the eyes, minus a
/// small dead zone around upright (no jump at its edge: the tilt grows from 0 there).
fn auto_tilt(pos: Vec3, eye: Vec3) -> f32 {
    let t = crate::world::panel::tilt_towards(pos, eye);
    t.signum() * (t.abs() - AUTO_TILT_DEAD).max(0.0)
}
const PPU_MAX: f32 = 3000.0;

fn modifiers(state: &SpaceState) -> ModifiersState {
    state.seat.get_keyboard().map(|k| k.modifier_state()).unwrap_or_default()
}

/// Holding this while carrying a panel disables wall snapping. Alt normally; nested,
/// Alt already is the mod key, so Ctrl.
fn snap_disabled(state: &SpaceState) -> bool {
    let m = modifiers(state);
    if state.backend.is_nested() {
        m.ctrl
    } else {
        m.alt
    }
}

/// The panel a panel command applies to: the one being carried, else the one under the
/// crosshair (front side, within reach), else the focused one.
pub fn target_panel(state: &SpaceState) -> Option<PanelId> {
    if let Some(g) = state.grab {
        return Some(g.panel);
    }
    let reach = state.input_settings.reach_distance;
    if let Some(hit) = state.hover.filter(|h| !h.from_back && h.t <= reach) {
        return Some(hit.panel);
    }
    match state.mode {
        InputMode::Window { panel } => Some(panel),
        InputMode::Walk => None,
    }
}

/// Mod+G: pick up the target panel, or put down the carried one.
pub fn toggle_grab(state: &mut SpaceState) {
    if state.grab.take().is_some() {
        tracing::debug!("panel dropped");
        state.layout_changed();
        return;
    }
    if let Some(panel) = target_panel(state) {
        grab_panel(state, panel);
    }
}

/// Pick up `panel` (it then follows the view until put down).
pub fn grab_panel(state: &mut SpaceState, panel: PanelId) {
    if matches!(state.mode, InputMode::Window { .. }) {
        set_mode(state, InputMode::Walk, None);
    }
    let Some(p) = state.world.panels().iter().find(|p| p.id == panel) else {
        return;
    };
    let distance = state.view.camera.position.distance(p.position).clamp(GRAB_MIN, GRAB_MAX);
    state.grab = Some(Grab { panel, distance, yaw_offset: 0.0, tilt_offset: 0.0 });
    tracing::debug!(panel, distance, "panel grabbed");
}

/// Scroll while carrying: nearer / farther.
pub fn grab_scroll(state: &mut SpaceState, notches: f64) {
    if let Some(g) = state.grab.as_mut() {
        // Each notch is 10% farther (or nearer): multiplicative, so it feels the same near and far.
        g.distance = (g.distance * 1.1f32.powf(notches as f32)).clamp(GRAB_MIN, GRAB_MAX);
        state.view.dirty = true;
        state.queue_redraw();
    }
}

/// Every frame while carrying: hold the panel in front of the camera, or snap it onto
/// the wall/floor the crosshair points at.
pub fn update_grab(state: &mut SpaceState) {
    let Some(g) = state.grab else {
        return;
    };
    let camera = state.view.camera;
    let snap = !snap_disabled(state);
    let Some(p) = state.world.panels().iter().find(|p| p.id == g.panel) else {
        state.grab = None;
        return;
    };
    let height = p.world_size().y;
    let auto = state.config.panels.auto_tilt;
    let clamp_tilt = |t: f32| t.clamp(-TILT_MAX, TILT_MAX);
    // Floating in the air: turned to the camera and (auto_tilt) leaning to face the eyes,
    // like a monitor turned towards you: put it high and it looks down at you.
    let face_camera = |pos: Vec3| {
        let d = camera.position - pos;
        let tilt = if auto { auto_tilt(pos, camera.position) } else { 0.0 };
        orientation(d.x.atan2(d.z) + g.yaw_offset, clamp_tilt(tilt + g.tilt_offset))
    };
    // On a wall or the ground only the tilt asked for by hand.
    let tilt = clamp_tilt(g.tilt_offset);

    let mut position = camera.position + camera.forward() * g.distance;
    let mut rotation = face_camera(position);
    if snap {
        let ray = raycast::camera_center_ray(&camera);
        let hit = raycast::voxel_ray(&ray, g.distance + 0.5, |c| state.voxels.solid(c));
        if let Some(VoxelHit { normal, t, .. }) = hit {
            let point = ray.origin + ray.dir * t;
            if normal.y == 0 && normal != glam::IVec3::ZERO {
                // A wall: flat against it, facing out along the face normal.
                // Tilted, its lower half would go into the wall: out by that much.
                let n = normal.as_vec3();
                position = point + n * (WALL_GAP + height / 2.0 * tilt.sin().abs());
                rotation = orientation(n.x.atan2(n.z) + g.yaw_offset, tilt);
            } else if normal.y > 0 {
                // The ground: stand the panel on it (a tilted one is a bit lower).
                position = point + Vec3::new(0.0, height / 2.0 * tilt.cos() + WALL_GAP, 0.0);
                let d = camera.position - position;
                rotation = orientation(d.x.atan2(d.z) + g.yaw_offset, tilt);
            }
        }
    }
    if let Some(p) = state.world.panels_mut().iter_mut().find(|p| p.id == g.panel) {
        if p.position != position || p.rotation != rotation {
            p.position = position;
            p.rotation = rotation;
            state.view.dirty = true;
        }
    }
}

/// Mod+R: turn 90°. Mod+Shift+R: face the camera again.
pub fn rotate_target(state: &mut SpaceState, reset: bool) {
    if let Some(g) = state.grab.as_mut() {
        g.yaw_offset = if reset { 0.0 } else { g.yaw_offset + std::f32::consts::FRAC_PI_2 };
        if reset {
            g.tilt_offset = 0.0;
        }
        state.queue_redraw();
        return;
    }
    if let Some(id) = target_panel(state) {
        rotate_panel(state, id, reset);
    }
}

/// Turn `id` by 90°, or (`reset`) make it face the camera again.
pub fn rotate_panel(state: &mut SpaceState, id: PanelId, reset: bool) {
    let camera = state.view.camera.position;
    if let Some(p) = state.world.panels_mut().iter_mut().find(|p| p.id == id) {
        if reset {
            p.face_towards(camera);
        } else {
            p.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) * p.rotation;
        }
    }
    state.view.dirty = true;
    state.layout_changed();
}

/// Mod+PageDown / Mod+PageUp: tilt the carried or targeted panel down (it looks down,
/// for a window up high) or up, by 15°.
pub fn tilt_target(state: &mut SpaceState, down: bool) {
    if let Some(g) = state.grab.as_mut() {
        let step = if down { TILT_STEP } else { -TILT_STEP };
        g.tilt_offset = (g.tilt_offset + step).clamp(-2.0 * TILT_MAX, 2.0 * TILT_MAX);
        state.queue_redraw();
        return;
    }
    if let Some(id) = target_panel(state) {
        tilt_panel(state, id, down);
    }
}

/// Tilt `id` one step down or up, keeping where it faces. The result is a whole number
/// of steps, so pressing back and forth returns exactly to upright.
pub fn tilt_panel(state: &mut SpaceState, id: PanelId, down: bool) {
    if let Some(p) = state.world.panels_mut().iter_mut().find(|p| p.id == id) {
        let (yaw, tilt) = p.yaw_tilt();
        let steps = (tilt / TILT_STEP).round() + if down { 1.0 } else { -1.0 };
        let tilt = (steps * TILT_STEP).clamp(-TILT_MAX, TILT_MAX);
        p.rotation = orientation(yaw, tilt);
        tracing::info!(panel = id, tilt_deg = tilt.to_degrees().round(), "panel tilted");
    }
    state.view.dirty = true;
    state.layout_changed();
}

/// Mod+Plus / Mod+Minus: the panel grows/shrinks by 10% (fewer/more pixels per metre).
pub fn scale_target(state: &mut SpaceState, bigger: bool) {
    if let Some(id) = target_panel(state) {
        scale_panel(state, id, bigger);
    }
}

/// `id` looks 10% bigger/smaller (the window keeps its pixel size).
pub fn scale_panel(state: &mut SpaceState, id: PanelId, bigger: bool) {
    if let Some(p) = state.world.panels_mut().iter_mut().find(|p| p.id == id) {
        let factor = if bigger { 1.0 / 1.1 } else { 1.1 };
        p.pixels_per_unit = (p.pixels_per_unit * factor).clamp(PPU_MIN, PPU_MAX);
        tracing::debug!(panel = id, ppu = p.pixels_per_unit, "panel scaled");
    }
    state.view.dirty = true;
    state.layout_changed();
}

/// Mod+H: hide the target panel; with nothing targeted, show all hidden panels again.
pub fn toggle_hidden(state: &mut SpaceState) {
    match target_panel(state) {
        Some(id) => hide_panel(state, id),
        None => {
            for p in state.world.panels_mut() {
                p.visible = true;
            }
        }
    }
    state.view.dirty = true;
    state.layout_changed();
}

/// Hide `id` (Mod+H with nothing targeted shows hidden panels again).
pub fn hide_panel(state: &mut SpaceState, id: PanelId) {
    if state.mode == (InputMode::Window { panel: id }) {
        set_mode(state, InputMode::Walk, None);
    }
    if state.grab.is_some_and(|g| g.panel == id) {
        state.grab = None;
    }
    if let Some(p) = state.world.panels_mut().iter_mut().find(|p| p.id == id) {
        p.visible = false;
    }
    state.view.dirty = true;
    state.layout_changed();
}

/// Ask a window's app to close (it may first ask about unsaved work).
pub fn close_window(window: &smithay::desktop::Window) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.send_close();
    } else if let Some(x11) = window.x11_surface() {
        let _ = x11.close();
    }
}

/// Make `id`'s window itself bigger/smaller (more/fewer rows and columns).
pub fn resize_panel(state: &mut SpaceState, id: PanelId, grow: bool) {
    let Some(window) = state.window_by_id(id) else { return };
    let size = window.geometry().size;
    let f = if grow { 1.1 } else { 1.0 / 1.1 };
    resize_window(
        &window,
        ((size.w as f64 * f).round() as i32, (size.h as f64 * f).round() as i32),
    );
    state.queue_redraw();
}

/// Left click on a block (Mod+left click anywhere): break the targeted block.
pub fn break_block(state: &mut SpaceState) {
    let Some(hit) = state.target_block else {
        return;
    };
    state.note_building();
    state.voxels.set_block(hit.block, Block::Air);
    state.layout.record_block(hit.block, Block::Air);
    state.layout_changed();
    state.queue_redraw();
}

/// Right click: place the selected block against the targeted face (never inside the
/// player).
pub fn place_block(state: &mut SpaceState) {
    let Some(hit) = state.target_block else {
        return;
    };
    let pos = hit.block + hit.normal;
    if hit.normal == glam::IVec3::ZERO
        || state.voxels.solid(pos)
        || state.player.overlaps_block(pos)
    {
        return;
    }
    let block = state.voxels.selected;
    state.note_building();
    state.voxels.set_block(pos, block);
    state.layout.record_block(pos, block);
    state.layout_changed();
    state.queue_redraw();
}

/// Window sizes spacewm asks apps for are kept within these (logical px). The top is what
/// any GPU can hold as one texture with room to spare (16384 is common): an 8192 px
/// wide window is four 1920 px screens, as big as anyone needs. Its buffer is 256 MB.
const MIN_WINDOW: (i32, i32) = (200, 120);
pub const MAX_WINDOW: (i32, i32) = (8192, 8192);

/// Mod+Ctrl+= / Mod+Ctrl+-: make the window itself bigger or smaller (more or fewer
/// terminal rows and columns), unlike Mod+= which only scales the panel in the world.
pub fn resize_target(state: &mut SpaceState, grow: bool) {
    let window = match state.render_mode {
        crate::cli::RenderMode::Scene3D => target_panel(state).and_then(|id| {
            state.world.panels().iter().find(|p| p.id == id).map(|p| p.window.clone())
        }),
        crate::cli::RenderMode::Flat2D => {
            match state.seat.get_keyboard().and_then(|k| k.current_focus()) {
                Some(crate::focus::KeyboardFocusTarget::Window(w)) => Some(w),
                _ => None,
            }
        }
    };
    let Some(window) = window else { return };
    let size = window.geometry().size;
    let f = if grow { 1.1 } else { 1.0 / 1.1 };
    let new = ((size.w as f64 * f).round() as i32, (size.h as f64 * f).round() as i32);
    resize_window(&window, new);
    state.queue_redraw();
}

/// Ask a window's app to take a new size (it answers by committing a buffer of that
/// size; the panel follows then, growing around its center). Callers queue a redraw: an
/// app waits for a frame callback before drawing at the new size, and an idle scene
/// sends none.
pub fn resize_window(window: &smithay::desktop::Window, (w, h): (i32, i32)) {
    let size = (w.clamp(MIN_WINDOW.0, MAX_WINDOW.0), h.clamp(MIN_WINDOW.1, MAX_WINDOW.1));
    tracing::debug!(w = size.0, h = size.1, "resize window");
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|s| s.size = Some(size.into()));
        toplevel.send_pending_configure();
    } else if let Some(x11) = window.x11_surface() {
        // X11 sizes include client-side shadows: keep them, change the visible part.
        let geo = x11.geometry();
        let mut rect = x11.last_configure();
        rect.size = (size.0 + rect.size.w - geo.size.w, size.1 + rect.size.h - geo.size.h).into();
        if let Err(e) = x11.configure(rect) {
            tracing::debug!("X11 resize failed: {e}");
        }
    }
}
