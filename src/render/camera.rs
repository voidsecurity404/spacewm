//! The first-person camera.
//!
//! Coordinates are right-handed, Y up, in metres (OpenGL convention): +X right, +Y up,
//! and the camera with yaw 0 looks toward −Z. The view matrix moves the world so the
//! camera sits at the origin looking down −Z; the projection matrix turns that into
//! clip space with perspective (far things smaller).

use glam::{Mat4, Vec3, Vec4};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    /// Rotation around Y in radians. 0 = looking toward −Z, positive turns right.
    pub yaw: f32,
    /// Up/down in radians, clamped to ±89°.
    pub pitch: f32,
    pub fov_y_deg: f32,
    pub near: f32,
    pub far: f32,
    pub aspect: f32,
}

const MAX_PITCH: f32 = 89.0 * std::f32::consts::PI / 180.0;

impl Default for Camera {
    fn default() -> Self {
        Self {
            // Eye height of a standing person, a few steps back from the origin.
            position: Vec3::new(0.0, 1.7, 3.0),
            yaw: 0.0,
            pitch: (-10.0f32).to_radians(),
            fov_y_deg: 75.0,
            near: 0.05,
            far: 500.0,
            aspect: 16.0 / 9.0,
        }
    }
}

impl Camera {
    /// Unit vector the camera looks along.
    pub fn forward(&self) -> Vec3 {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(cp * sy, sp, -cp * cy).normalize()
    }

    /// Unit vector to the camera's right, always horizontal.
    #[allow(dead_code)] // used by raycasting (Stage 4)
    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalize()
    }

    #[allow(dead_code)] // used by raycasting (Stage 4)
    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward())
    }

    /// Forward projected onto the ground plane (for walking: looking down doesn't slow you).
    pub fn forward_flat(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(sy, 0.0, -cy)
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_to_rh(self.position, self.forward(), Vec3::Y)
    }

    /// `perspective_rh_gl`: right-handed, depth mapped to −1..1 as OpenGL expects.
    pub fn projection_matrix(&self) -> Mat4 {
        Mat4::perspective_rh_gl(self.fov_y_deg.to_radians(), self.aspect, self.near, self.far)
    }

    /// Mouse look. Moving the mouse right (dx > 0) turns right; moving it down (dy > 0,
    /// screen y grows downward) looks down.
    pub fn rotate(&mut self, dx: f32, dy: f32, sensitivity: f32) {
        self.yaw = (self.yaw + dx * sensitivity).rem_euclid(std::f32::consts::TAU);
        // Clamping short of ±90° keeps forward × up well defined, so the view never flips.
        self.pitch = (self.pitch - dy * sensitivity).clamp(-MAX_PITCH, MAX_PITCH);
    }

    pub fn set_aspect(&mut self, width: i32, height: i32) {
        if width > 0 && height > 0 {
            self.aspect = width as f32 / height as f32;
        }
    }

    /// Ray through a point on screen in normalized device coordinates (−1..1, y up):
    /// returns (origin on the near plane, normalized direction).
    #[allow(dead_code)] // used by raycasting (Stage 4)
    pub fn screen_ray(&self, ndc_x: f32, ndc_y: f32) -> (Vec3, Vec3) {
        let inv = (self.projection_matrix() * self.view_matrix()).inverse();
        let unproject = |z: f32| {
            let p = inv * Vec4::new(ndc_x, ndc_y, z, 1.0);
            p.truncate() / p.w
        };
        let near = unproject(-1.0);
        let far = unproject(1.0);
        (near, (far - near).normalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    #[test]
    fn yaw_zero_looks_down_negative_z() {
        let cam = Camera { pitch: 0.0, ..Camera::default() };
        assert!(close(cam.forward(), Vec3::NEG_Z));
        assert!(close(cam.right(), Vec3::X));
        assert!(close(cam.up(), Vec3::Y));
    }

    #[test]
    fn basis_is_orthonormal() {
        for (yaw, pitch) in [(0.3, 0.2), (2.0, -1.2), (5.5, 1.5)] {
            let cam = Camera { yaw, pitch, ..Camera::default() };
            let (f, r, u) = (cam.forward(), cam.right(), cam.up());
            assert!(f.dot(r).abs() < 1e-5 && f.dot(u).abs() < 1e-5 && r.dot(u).abs() < 1e-5);
            assert!((f.length() - 1.0).abs() < 1e-5);
            assert!((r.length() - 1.0).abs() < 1e-5);
            assert!((u.length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn positive_yaw_turns_right() {
        let mut cam = Camera { pitch: 0.0, ..Camera::default() };
        cam.rotate(100.0, 0.0, std::f32::consts::FRAC_PI_2 / 100.0);
        assert!(close(cam.forward(), Vec3::X));
    }

    #[test]
    fn pitch_is_clamped() {
        let mut cam = Camera::default();
        cam.rotate(0.0, -1.0e6, 0.01);
        assert!((cam.pitch - MAX_PITCH).abs() < 1e-6);
        cam.rotate(0.0, 1.0e6, 0.01);
        assert!((cam.pitch + MAX_PITCH).abs() < 1e-6);
        // Even at the clamp the basis is still valid (no NaN from a degenerate cross).
        assert!(cam.right().is_finite());
    }

    #[test]
    fn aspect_ignores_zero_height() {
        let mut cam = Camera::default();
        cam.set_aspect(800, 0);
        assert_eq!(cam.aspect, 16.0 / 9.0);
        cam.set_aspect(800, 400);
        assert_eq!(cam.aspect, 2.0);
    }

    #[test]
    fn projects_a_known_point_to_screen_center() {
        let cam = Camera { position: Vec3::ZERO, pitch: 0.0, ..Camera::default() };
        let clip = cam.projection_matrix() * cam.view_matrix() * Vec4::new(0.0, 0.0, -10.0, 1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1e-5 && ndc.y.abs() < 1e-5);
        assert!(ndc.z > -1.0 && ndc.z < 1.0, "inside the depth range");
        // A point at the top edge of the vertical FOV lands at ndc y = 1.
        let half = (cam.fov_y_deg / 2.0).to_radians();
        let top = Vec4::new(0.0, 10.0 * half.tan(), -10.0, 1.0);
        let clip = cam.projection_matrix() * cam.view_matrix() * top;
        assert!((clip.y / clip.w - 1.0).abs() < 1e-4);
    }

    #[test]
    fn center_screen_ray_matches_forward() {
        let cam = Camera { yaw: 0.7, pitch: -0.3, ..Camera::default() };
        let (origin, dir) = cam.screen_ray(0.0, 0.0);
        assert!(close(dir, cam.forward()));
        assert!((origin - cam.position).length() < cam.near * 1.5);
    }
}
