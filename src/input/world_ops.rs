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
    world::{block::Block, panel::PanelId},
};

/// A panel being carried.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grab {
    pub panel: PanelId,
    /// How far in front of the eyes it floats (scroll wheel changes it).
    pub distance: f32,
    /// Extra rotation around Y on top of "facing the camera" (Mod+R).
    pub yaw_offset: f32,
}

const GRAB_MIN: f32 = 0.8;
const GRAB_MAX: f32 = 8.0;
/// A panel hung on a wall floats this far in front of it (no z-fighting with the wall).
const WALL_GAP: f32 = 0.02;
const PPU_MIN: f32 = 200.0;
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
fn target_panel(state: &SpaceState) -> Option<PanelId> {
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
    let Some(panel) = target_panel(state) else {
        return;
    };
    if matches!(state.mode, InputMode::Window { .. }) {
        set_mode(state, InputMode::Walk, None);
    }
    let Some(p) = state.world.panels().iter().find(|p| p.id == panel) else {
        return;
    };
    let distance = state.view.camera.position.distance(p.position).clamp(GRAB_MIN, GRAB_MAX);
    state.grab = Some(Grab { panel, distance, yaw_offset: 0.0 });
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
    let face_camera = |pos: Vec3| {
        let d = camera.position - pos;
        Quat::from_rotation_y(d.x.atan2(d.z) + g.yaw_offset)
    };

    let mut position = camera.position + camera.forward() * g.distance;
    let mut rotation = face_camera(position);
    if snap {
        let ray = raycast::camera_center_ray(&camera);
        let hit = raycast::voxel_ray(&ray, g.distance + 0.5, |c| state.voxels.solid(c));
        if let Some(VoxelHit { normal, t, .. }) = hit {
            let point = ray.origin + ray.dir * t;
            if normal.y == 0 && normal != glam::IVec3::ZERO {
                // A wall: flat against it, facing out along the face normal.
                let n = normal.as_vec3();
                position = point + n * WALL_GAP;
                rotation = Quat::from_rotation_y(n.x.atan2(n.z) + g.yaw_offset);
            } else if normal.y > 0 {
                // The ground: stand the panel on it.
                position = point + Vec3::new(0.0, height / 2.0 + WALL_GAP, 0.0);
                rotation = face_camera(position);
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
        state.queue_redraw();
        return;
    }
    let Some(id) = target_panel(state) else {
        return;
    };
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

/// Mod+Plus / Mod+Minus: the panel grows/shrinks by 10% (fewer/more pixels per metre).
pub fn scale_target(state: &mut SpaceState, bigger: bool) {
    let Some(id) = target_panel(state) else {
        return;
    };
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
        Some(id) => {
            if state.mode == (InputMode::Window { panel: id }) {
                set_mode(state, InputMode::Walk, None);
            }
            if state.grab.is_some_and(|g| g.panel == id) {
                state.grab = None;
            }
            if let Some(p) = state.world.panels_mut().iter_mut().find(|p| p.id == id) {
                p.visible = false;
            }
        }
        None => {
            for p in state.world.panels_mut() {
                p.visible = true;
            }
        }
    }
    state.view.dirty = true;
    state.layout_changed();
}

/// Mod+left click: break the targeted block.
pub fn break_block(state: &mut SpaceState) {
    let Some(hit) = state.target_block else {
        return;
    };
    state.voxels.set_block(hit.block, Block::Air);
    state.layout.record_block(hit.block, Block::Air);
    state.layout_changed();
    state.queue_redraw();
}

/// Mod+right click: place the selected block against the targeted face (never inside
/// the player).
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
    state.voxels.set_block(pos, block);
    state.layout.record_block(pos, block);
    state.layout_changed();
    state.queue_redraw();
}

pub fn select_block(state: &mut SpaceState, index: usize) {
    if let Some(&b) = Block::PLACEABLE.get(index) {
        state.voxels.selected = b;
        tracing::info!(block = ?b, "block selected");
    }
}
