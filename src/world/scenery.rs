//! What the world around you is made of: the map (world/map.rs) or the block world
//! (world/voxels.rs). The rest of spacewm asks here (where to spawn, what a ray hits,
//! what the player bumps into) and doesn't care which one it is.

use std::rc::Rc;

use glam::Vec3;

use super::{map::Map, player::Solids};
use crate::{config::WorldMap, input::raycast::Ray, state::SpaceState};

/// Steps up to this high are walked up on the map (its stairs are 0.35 m).
const MAP_STEP: f32 = 0.45;

impl Solids for Map {
    fn boxes(&self, min: Vec3, max: Vec3, out: &mut Vec<(Vec3, Vec3)>) {
        Map::boxes(self, min, max, out);
    }

    fn step_height(&self) -> f32 {
        MAP_STEP
    }
}

/// A ray hit on the world: how far, and the face normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldHit {
    pub t: f32,
    pub normal: Vec3,
}

impl SpaceState {
    /// Use `which` as the world: load the map (or drop it for the block world) and put
    /// the player at its start. Called at startup and when the config changes it.
    pub fn set_world_map(&mut self, which: WorldMap) {
        let want_map = which == WorldMap::Dust;
        if want_map == self.map.is_some() {
            return;
        }
        // Open windows keep their spot in the world they leave, for when you come back.
        let old = self.world_name();
        for p in self.world.panels() {
            if let Some(saved) = crate::state::saved_panel(p, old) {
                self.layout.remember(saved);
            }
        }
        if want_map {
            match Map::load(super::map::DUST) {
                Ok(map) => self.map = Some(Rc::new(map)),
                Err(e) => {
                    tracing::error!("map not loaded, staying in the block world: {e}");
                    self.show_error(vec!["The map could not be loaded:".into(), e]);
                    return;
                }
            }
        } else {
            self.map = None;
        }
        self.map_generation += 1;
        tracing::info!(map = ?which, "world");
        self.respawn();
        self.bring_windows_along();
        self.layout_changed();
        self.view.dirty = true;
        self.queue_redraw();
    }

    /// After a world change: every open window goes where it was in this world before,
    /// or (never here before) onto an arc in front of you.
    fn bring_windows_along(&mut self) {
        let world = self.world_name();
        let mut front = Vec::new();
        let ids: Vec<_> = self.world.panels().iter().map(|p| p.id).collect();
        for id in ids {
            let Some(p) = self.world.panels().iter().find(|p| p.id == id) else { continue };
            let (app, title) = crate::state::window_names(&p.window);
            let app = app.or_else(|| p.names.0.clone());
            let saved = app.and_then(|a| self.layout.take_placement(&a, title.as_deref(), world));
            let width = p.world_size().x;
            let size = p.size_px;
            match saved {
                Some(s) => {
                    if let Some(p) = self.world.panels_mut().iter_mut().find(|p| p.id == id) {
                        p.position = s.position();
                        p.rotation = s.rotation();
                        p.pixels_per_unit = s.pixels_per_unit;
                    }
                }
                None => front.push((id, width, size)),
            }
        }
        if front.is_empty() {
            return;
        }
        let eye = self.view.camera.position;
        let radius = front.iter().map(|(_, _, s)| self.spawn_distance(*s)).fold(0.0f32, f32::max);
        let widths: Vec<f32> = front.iter().map(|(_, w, _)| *w).collect();
        let (yaws, r) = crate::input::seated::arc(&widths, self.view.camera.yaw, radius);
        for ((id, _, _), yaw) in front.iter().zip(yaws) {
            let pos = eye + Vec3::new(yaw.sin(), 0.0, -yaw.cos()) * r;
            if let Some(p) = self.world.panels_mut().iter_mut().find(|p| p.id == *id) {
                p.position = pos;
                p.face_towards(eye);
            }
        }
    }

    /// The world's name in saved files: "dust" or "voxel".
    pub fn world_name(&self) -> &'static str {
        if self.map.is_some() {
            "dust"
        } else {
            "voxel"
        }
    }

    /// Where you start, and which way you look (radians).
    pub fn spawn_point(&self) -> (Vec3, f32) {
        match &self.map {
            Some(m) => (m.spawn, m.spawn_yaw_deg.to_radians()),
            None => (self.voxels.spawn, self.voxels.spawn_yaw),
        }
    }

    /// Back to the start (new world, or fell off it).
    pub fn respawn(&mut self) {
        let (pos, yaw) = self.spawn_point();
        let sprint = self.player.sprint_speed;
        self.player = super::player::Player::new(pos);
        self.player.sprint_speed = sprint;
        self.view.camera.position = self.player.eye();
        self.view.camera.yaw = yaw;
        self.view.camera.pitch = 0.0;
    }

    /// The first solid thing (wall, floor, block) a ray meets within `max`.
    pub fn world_ray(&self, ray: &Ray, max: f32) -> Option<WorldHit> {
        match &self.map {
            Some(m) => m.ray(ray.origin, ray.dir, max).map(|(t, normal)| WorldHit { t, normal }),
            None => crate::input::raycast::voxel_ray(ray, max, |c| self.voxels.solid(c))
                .map(|h| WorldHit { t: h.t, normal: h.normal.as_vec3() }),
        }
    }
}
