//! View-frustum culling: skip chunks and panels the camera can't see.
//!
//! The frustum is the pyramid the camera sees, bounded by 6 planes. They come straight
//! out of the combined projection·view matrix (Gribb & Hartmann): each plane is the
//! last matrix row plus or minus one of the others. A box is invisible if it lies
//! entirely behind any one plane.

use glam::{Mat4, Vec3, Vec4};

pub struct Frustum {
    /// (normal.xyz, d): points p with dot(n, p) + d >= 0 are inside.
    planes: [Vec4; 6],
}

impl Frustum {
    pub fn from_matrix(proj_view: Mat4) -> Self {
        let row = |i: usize| proj_view.row(i);
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let mut planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2];
        for p in &mut planes {
            let len = p.truncate().length();
            if len > 0.0 {
                *p /= len;
            }
        }
        Self { planes }
    }

    /// Is any part of the axis-aligned box [min, max] inside the frustum?
    pub fn aabb_visible(&self, min: Vec3, max: Vec3) -> bool {
        self.planes.iter().all(|p| {
            // The box corner farthest along the plane normal ("positive vertex"): if even
            // that one is behind the plane, the whole box is.
            let v = Vec3::new(
                if p.x >= 0.0 { max.x } else { min.x },
                if p.y >= 0.0 { max.y } else { min.y },
                if p.z >= 0.0 { max.z } else { min.z },
            );
            p.truncate().dot(v) + p.w >= 0.0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::camera::Camera;

    fn frustum(cam: &Camera) -> Frustum {
        Frustum::from_matrix(cam.projection_matrix() * cam.view_matrix())
    }

    #[test]
    fn boxes_in_front_and_behind() {
        let cam = Camera { position: Vec3::ZERO, yaw: 0.0, pitch: 0.0, ..Camera::default() };
        let f = frustum(&cam);
        // In front (−Z).
        assert!(f.aabb_visible(Vec3::new(-1.0, -1.0, -11.0), Vec3::new(1.0, 1.0, -9.0)));
        // Behind the camera.
        assert!(!f.aabb_visible(Vec3::new(-1.0, -1.0, 9.0), Vec3::new(1.0, 1.0, 11.0)));
        // Far to the side.
        assert!(!f.aabb_visible(Vec3::new(100.0, -1.0, -3.0), Vec3::new(102.0, 1.0, -1.0)));
        // Beyond the far plane.
        assert!(!f.aabb_visible(Vec3::new(-1.0, -1.0, -900.0), Vec3::new(1.0, 1.0, -800.0)));
        // A huge box around the camera is visible.
        assert!(f.aabb_visible(Vec3::splat(-50.0), Vec3::splat(50.0)));
    }

    #[test]
    fn turning_changes_what_is_visible() {
        let mut cam = Camera { position: Vec3::ZERO, yaw: 0.0, pitch: 0.0, ..Camera::default() };
        let right_box = (Vec3::new(9.0, -1.0, -1.0), Vec3::new(11.0, 1.0, 1.0));
        assert!(!frustum(&cam).aabb_visible(right_box.0, right_box.1));
        cam.yaw = std::f32::consts::FRAC_PI_2; // look toward +X
        assert!(frustum(&cam).aabb_visible(right_box.0, right_box.1));
    }
}
