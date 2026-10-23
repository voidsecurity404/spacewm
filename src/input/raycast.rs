//! Raycasting: which panel is the user looking (or pointing) at, and where on it.
//!
//! A ray is origin + t·dir (t ≥ 0, dir normalized). A panel is a rectangle in a plane,
//! so hitting it is: intersect the ray with the plane, move the hit point into the
//! panel's own coordinates (inverse model matrix), and check it lies inside the unit
//! square. Pure math: everything here is unit tested.

use glam::{IVec3, Vec3};
use smithay::utils::{Logical, Point};

use crate::{
    render::camera::Camera,
    world::{
        panel::{Panel, PanelId},
        World,
    },
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    /// Normalized.
    pub dir: Vec3,
}

/// The ray through the crosshair (screen center).
pub fn camera_center_ray(camera: &Camera) -> Ray {
    Ray { origin: camera.position, dir: camera.forward() }
}

/// The ray through a point on the output (logical px, top-left origin).
pub fn camera_screen_ray(camera: &Camera, pos: Point<f64, Logical>, size: (i32, i32)) -> Ray {
    // Normalized device coordinates: −1..1 left to right and BOTTOM to top.
    let ndc_x = (2.0 * pos.x / size.0.max(1) as f64 - 1.0) as f32;
    let ndc_y = (1.0 - 2.0 * pos.y / size.1.max(1) as f64) as f32;
    let (origin, dir) = camera.screen_ray(ndc_x, ndc_y);
    Ray { origin, dir }
}

/// Distance t along the ray to the plane through `point` with normal `normal`, or None if
/// the ray is parallel to the plane or the plane is behind the ray's origin.
///
/// Points p on the plane satisfy dot(p − point, n) = 0. Substituting p = o + t·d gives
/// t = dot(point − o, n) / dot(d, n).
pub fn ray_plane(ray: &Ray, point: Vec3, normal: Vec3) -> Option<f32> {
    let denom = ray.dir.dot(normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (point - ray.origin).dot(normal) / denom;
    (t >= 0.0).then_some(t)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelHit {
    pub panel: PanelId,
    pub t: f32,
    pub world_point: Vec3,
    /// Hit position in window-geometry pixels (0,0 = top-left of the window content).
    pub surface_px: Point<f64, Logical>,
    /// The ray hit the panel's back side (it can't be clicked from behind).
    pub from_back: bool,
}

/// Where `ray` hits `panel`'s rectangle, if it does.
pub fn ray_panel<W>(ray: &Ray, panel: &Panel<W>) -> Option<PanelHit> {
    let normal = panel.normal();
    let t = ray_plane(ray, panel.position, normal)?;
    let world_point = ray.origin + ray.dir * t;
    // Into panel space, where the rectangle is the unit square −0.5..0.5 in XY.
    let local = panel.model_matrix().inverse().transform_point3(world_point);
    if local.x.abs() > 0.5 || local.y.abs() > 0.5 {
        return None;
    }
    let (w, h) = (panel.size_px.0 as f64, panel.size_px.1 as f64);
    // Wayland y grows downward, panel y grows upward.
    let surface_px = Point::from(((local.x as f64 + 0.5) * w, (0.5 - local.y as f64) * h));
    Some(PanelHit {
        panel: panel.id,
        t,
        world_point,
        surface_px,
        // The ray travels along the normal = it comes from behind.
        from_back: ray.dir.dot(normal) > 0.0,
    })
}

/// The nearest visible panel along the ray within `max_distance`. Back-side hits are
/// returned too (they block what is behind them); callers decide what they allow.
pub fn pick<W: PartialEq>(ray: &Ray, world: &World<W>, max_distance: f32) -> Option<PanelHit> {
    world
        .panels()
        .iter()
        .filter(|p| p.visible)
        .filter_map(|p| ray_panel(ray, p))
        .filter(|hit| hit.t <= max_distance)
        .min_by(|a, b| a.t.total_cmp(&b.t))
}

/// A block hit by a ray: the block cell, the face it entered through (outward normal),
/// and the distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoxelHit {
    pub block: IVec3,
    pub normal: IVec3,
    pub t: f32,
}

/// Walk the ray through the block grid one cell at a time (Amanatides & Woo "DDA") and
/// return the first cell where `solid` is true, within `max_distance`.
///
/// For each axis we track `t_max`: how far along the ray the next cell boundary on that
/// axis is. Stepping into the neighbor across the nearest boundary visits exactly the
/// cells the ray passes through, in order, with no gaps and no repeats.
pub fn voxel_ray(ray: &Ray, max_distance: f32, solid: impl Fn(IVec3) -> bool) -> Option<VoxelHit> {
    let mut cell = ray.origin.floor().as_ivec3();
    if solid(cell) {
        return Some(VoxelHit { block: cell, normal: IVec3::ZERO, t: 0.0 });
    }
    let step =
        IVec3::new(ray.dir.x.signum() as i32, ray.dir.y.signum() as i32, ray.dir.z.signum() as i32);
    let mut t_max = Vec3::ZERO;
    let mut t_delta = Vec3::ZERO;
    for axis in 0..3 {
        let d = ray.dir[axis];
        if d.abs() < 1e-9 {
            t_max[axis] = f32::INFINITY;
            t_delta[axis] = f32::INFINITY;
        } else {
            let boundary = if d > 0.0 { cell[axis] as f32 + 1.0 } else { cell[axis] as f32 };
            t_max[axis] = (boundary - ray.origin[axis]) / d;
            t_delta[axis] = 1.0 / d.abs();
        }
    }
    loop {
        // The nearest boundary decides which axis we cross.
        let axis = if t_max.x < t_max.y {
            if t_max.x < t_max.z {
                0
            } else {
                2
            }
        } else if t_max.y < t_max.z {
            1
        } else {
            2
        };
        let t = t_max[axis];
        if t > max_distance {
            return None;
        }
        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        if solid(cell) {
            let mut normal = IVec3::ZERO;
            normal[axis] = -step[axis];
            return Some(VoxelHit { block: cell, normal, t });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    fn panel(id: PanelId, position: Vec3, yaw: f32) -> Panel<()> {
        let mut p = Panel::new(id, (), (800, 400));
        p.position = position;
        p.rotation = Quat::from_rotation_y(yaw);
        p
    }

    fn ray(origin: Vec3, target: Vec3) -> Ray {
        Ray { origin, dir: (target - origin).normalize() }
    }

    #[test]
    fn plane_intersection() {
        let r = Ray { origin: Vec3::ZERO, dir: Vec3::NEG_Z };
        assert_eq!(ray_plane(&r, Vec3::new(0.0, 0.0, -3.0), Vec3::Z), Some(3.0));
        // Parallel.
        assert_eq!(
            ray_plane(
                &Ray { origin: Vec3::ZERO, dir: Vec3::X },
                Vec3::new(0.0, 0.0, -3.0),
                Vec3::Z
            ),
            None
        );
        // Behind the origin.
        assert_eq!(ray_plane(&r, Vec3::new(0.0, 0.0, 3.0), Vec3::Z), None);
    }

    #[test]
    fn center_hit_maps_to_window_center() {
        let p = panel(1, Vec3::new(0.0, 1.7, -2.0), 0.0);
        let hit = ray_panel(&ray(Vec3::new(0.0, 1.7, 0.0), p.position), &p).expect("hit");
        assert!((hit.surface_px.x - 400.0).abs() < 1e-3 && (hit.surface_px.y - 200.0).abs() < 1e-3);
        assert!((hit.t - 2.0).abs() < 1e-5);
        assert!(!hit.from_back);
    }

    #[test]
    fn top_left_corner_is_origin() {
        let p = panel(1, Vec3::new(0.0, 1.7, -2.0), 0.0);
        // 800x400 px at 800 px/m = 1 m x 0.5 m; aim just inside the top-left corner.
        let corner = Vec3::new(-0.5 + 1e-4, 1.7 + 0.25 - 1e-4, -2.0);
        let hit = ray_panel(&ray(Vec3::new(0.0, 1.7, 0.0), corner), &p).expect("hit");
        assert!(hit.surface_px.x < 0.5 && hit.surface_px.y < 0.5, "{:?}", hit.surface_px);
        // Just outside misses.
        let outside = Vec3::new(-0.51, 1.7, -2.0);
        assert!(ray_panel(&ray(Vec3::new(0.0, 1.7, 0.0), outside), &p).is_none());
    }

    #[test]
    fn rotated_panel() {
        // Panel at +X facing -X (yaw -90°): its front looks back at the origin.
        let p = panel(1, Vec3::new(3.0, 1.0, 0.0), -std::f32::consts::FRAC_PI_2);
        assert!((p.normal() - Vec3::NEG_X).length() < 1e-5);
        // The panel's local +X points toward world +Z here, which is also the right-hand
        // side of a viewer at the origin looking toward +X: aiming 0.25 m toward +Z hits
        // the window's right half.
        let hit =
            ray_panel(&ray(Vec3::new(0.0, 1.0, 0.0), Vec3::new(3.0, 1.0, 0.25)), &p).expect("hit");
        assert!((hit.surface_px.x - 600.0).abs() < 1.0, "{:?}", hit.surface_px);
        assert!(!hit.from_back);
    }

    #[test]
    fn parallel_ray_misses() {
        let p = panel(1, Vec3::new(0.0, 1.7, -2.0), 0.0);
        let r = Ray { origin: Vec3::new(-5.0, 1.7, -2.0), dir: Vec3::X };
        assert!(ray_panel(&r, &p).is_none());
    }

    #[test]
    fn from_behind() {
        let p = panel(1, Vec3::new(0.0, 1.7, -2.0), 0.0);
        let hit = ray_panel(&ray(Vec3::new(0.0, 1.7, -4.0), p.position), &p).expect("hit");
        assert!(hit.from_back);
        // From behind, the viewer's left is the window's right: x is mirrored.
        let hit = ray_panel(&ray(Vec3::new(0.0, 1.7, -4.0), Vec3::new(0.25, 1.7, -2.0)), &p)
            .expect("hit");
        assert!((hit.surface_px.x - 600.0).abs() < 1.0);
    }

    #[test]
    fn nearer_panel_wins_and_reach_is_respected() {
        let mut world = World::<u32>::default();
        let cam = Camera { position: Vec3::new(0.0, 1.7, 0.0), pitch: 0.0, ..Camera::default() };
        world.add_window(1, 1, (800, 400), &cam, 4.0);
        world.add_window(2, 2, (800, 400), &cam, 4.0);
        // Put them in a line in front of the camera.
        for p in world.panels_mut() {
            p.position = Vec3::new(0.0, 1.7, if p.id == 1 { -4.0 } else { -2.0 });
            p.rotation = Quat::IDENTITY;
        }
        let r = camera_center_ray(&cam);
        assert_eq!(pick(&r, &world, 20.0).map(|h| h.panel), Some(2));
        assert_eq!(pick(&r, &world, 1.5), None, "out of reach");
        // Hidden panels are ignored.
        world.panels_mut()[1].visible = false;
        assert_eq!(pick(&r, &world, 20.0).map(|h| h.panel), Some(1));
    }

    #[test]
    fn voxel_ray_hits_the_first_block_and_its_face() {
        // A wall of blocks at x = 5.
        let wall = |c: IVec3| c.x == 5;
        let r = Ray { origin: Vec3::new(0.5, 0.5, 0.5), dir: Vec3::X };
        let hit = voxel_ray(&r, 10.0, wall).expect("hit");
        assert_eq!(hit.block, IVec3::new(5, 0, 0));
        assert_eq!(hit.normal, IVec3::new(-1, 0, 0), "entered through the -X face");
        assert!((hit.t - 4.5).abs() < 1e-5);
        assert!(voxel_ray(&r, 4.0, wall).is_none(), "out of reach");
        // Diagonal down onto a floor at y = 0.
        let floor = |c: IVec3| c.y <= 0;
        let r =
            Ray { origin: Vec3::new(0.2, 3.5, 0.2), dir: Vec3::new(1.0, -1.0, 0.5).normalize() };
        let hit = voxel_ray(&r, 20.0, floor).expect("hit");
        assert_eq!(hit.normal, IVec3::new(0, 1, 0), "came in from above");
        assert_eq!(hit.block.y, 0);
        // The hit point really is on the top face of that block.
        let p = r.origin + r.dir * hit.t;
        assert!((p.y - 1.0).abs() < 1e-4 && p.x.floor() as i32 == hit.block.x);
    }

    #[test]
    fn voxel_ray_negative_directions() {
        let wall = |c: IVec3| c == IVec3::new(-3, 1, -2);
        let target = Vec3::new(-2.5, 1.5, -1.5);
        let origin = Vec3::new(1.2, 1.5, 1.7);
        let r = Ray { origin, dir: (target - origin).normalize() };
        let hit = voxel_ray(&r, 20.0, wall).expect("hit");
        assert_eq!(hit.block, IVec3::new(-3, 1, -2));
    }

    #[test]
    fn screen_ray_through_center_matches_center_ray() {
        let cam = Camera { yaw: 0.4, pitch: -0.2, ..Camera::default() };
        let r = camera_screen_ray(&cam, Point::from((640.0, 360.0)), (1280, 720));
        assert!((r.dir - camera_center_ray(&cam).dir).length() < 1e-4);
        // A point to the right of center turns the ray right.
        let right = camera_screen_ray(&cam, Point::from((1200.0, 360.0)), (1280, 720));
        assert!(right.dir.dot(cam.right()) > 0.0);
    }
}
