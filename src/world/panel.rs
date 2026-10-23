//! A panel: one application window standing in 3D space.
//!
//! The panel's rectangle is the window's *geometry* (its visible content, without
//! client-side shadows). Its center sits at `position`, it faces its local +Z, and its
//! size in metres is the window size in pixels divided by `pixels_per_unit`.
//!
//! The texture drawn on it covers the window's whole bounding box including popups and
//! shadows (`bbox_px`), which can be larger than the geometry; `bbox_model_matrix` places
//! that bigger quad so the geometry part lines up exactly with the panel rectangle.

use glam::{Mat4, Quat, Vec2, Vec3};
use smithay::{
    desktop::Window,
    utils::{Logical, Rectangle},
};

pub type PanelId = u64;

/// 800 px per metre: a 1000 px wide terminal is 1.25 m wide, about a big monitor.
pub const DEFAULT_PIXELS_PER_UNIT: f32 = 800.0;

/// Generic over the window type only so the math can be unit-tested without a live
/// Wayland client (tests use `Panel<()>`).
#[derive(Debug, Clone)]
pub struct Panel<W = Window> {
    pub id: PanelId,
    pub window: W,
    /// World position of the panel CENTER (center of the window geometry).
    pub position: Vec3,
    /// Orientation; the front of the panel faces its local +Z.
    pub rotation: Quat,
    pub pixels_per_unit: f32,
    /// Window geometry size in logical px.
    pub size_px: (i32, i32),
    /// Window bounding box (with popups) relative to the geometry's top-left corner.
    pub bbox_px: Rectangle<i32, Logical>,
    pub opacity: f32,
    pub visible: bool,
    /// The client committed new content: the texture must be re-rendered.
    pub content_dirty: bool,
    /// We don't know the app yet (no app_id): a saved placement may still be applied.
    pub placement_pending: bool,
    /// X11 menus/tooltips (override-redirect windows) drawn into this panel's texture.
    pub x11_children: Vec<crate::xwayland::X11Child>,
}

impl<W> Panel<W> {
    pub fn new(id: PanelId, window: W, size_px: (i32, i32)) -> Self {
        Self {
            id,
            window,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            pixels_per_unit: DEFAULT_PIXELS_PER_UNIT,
            size_px,
            bbox_px: Rectangle::from_size(size_px.into()),
            opacity: 1.0,
            visible: true,
            content_dirty: true,
            placement_pending: true,
            x11_children: Vec::new(),
        }
    }

    /// Width and height in metres.
    pub fn world_size(&self) -> Vec2 {
        Vec2::new(self.size_px.0 as f32, self.size_px.1 as f32) / self.pixels_per_unit
    }

    /// Unit square (−0.5..0.5 in local XY) → the panel rectangle in the world.
    pub fn model_matrix(&self) -> Mat4 {
        let size = self.world_size();
        Mat4::from_scale_rotation_translation(
            Vec3::new(size.x, size.y, 1.0),
            self.rotation,
            self.position,
        )
    }

    /// Unit square → the textured quad covering the whole bounding box.
    pub fn bbox_model_matrix(&self) -> Mat4 {
        let ppu = self.pixels_per_unit;
        let (gw, gh) = (self.size_px.0 as f32, self.size_px.1 as f32);
        let b = self.bbox_px;
        // Bbox center relative to the geometry center, in px with Y pointing UP (pixel
        // rows grow downward, world Y grows upward).
        let cx = b.loc.x as f32 + b.size.w as f32 / 2.0 - gw / 2.0;
        let cy = gh / 2.0 - (b.loc.y as f32 + b.size.h as f32 / 2.0);
        Mat4::from_rotation_translation(self.rotation, self.position)
            * Mat4::from_translation(Vec3::new(cx / ppu, cy / ppu, 0.0))
            * Mat4::from_scale(Vec3::new(b.size.w as f32 / ppu, b.size.h as f32 / ppu, 1.0))
    }

    /// The direction the front side faces.
    pub fn normal(&self) -> Vec3 {
        self.rotation * Vec3::Z
    }

    /// Corners in world space: top-left, top-right, bottom-right, bottom-left.
    #[allow(dead_code)] // used by raycast debugging (Stage 4)
    pub fn corners_world(&self) -> [Vec3; 4] {
        let m = self.model_matrix();
        [(-0.5, 0.5), (0.5, 0.5), (0.5, -0.5), (-0.5, -0.5)]
            .map(|(x, y)| m.transform_point3(Vec3::new(x, y, 0.0)))
    }

    /// Turn (around Y only, staying upright) so the front faces `target`.
    ///
    /// A rotation by angle θ around Y maps +Z to (sin θ, 0, cos θ). We want that to point
    /// along the horizontal direction d = target − position, so θ = atan2(d.x, d.z).
    pub fn face_towards(&mut self, target: Vec3) {
        let d = target - self.position;
        if d.x.abs() < 1e-6 && d.z.abs() < 1e-6 {
            return; // target straight above/below: any yaw works, keep the current one
        }
        self.rotation = Quat::from_rotation_y(d.x.atan2(d.z));
    }

    /// Resize to a new geometry size, keeping the center where it is.
    pub fn set_size(&mut self, size_px: (i32, i32)) {
        self.size_px = size_px;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(size: (i32, i32)) -> Panel<()> {
        let mut p = Panel::new(1, (), size);
        p.position = Vec3::new(0.0, 1.7, -2.0);
        p
    }

    #[test]
    fn world_size_from_pixels() {
        let p = panel((1000, 700));
        assert_eq!(p.world_size(), Vec2::new(1.25, 0.875));
    }

    #[test]
    fn model_matrix_maps_unit_square_to_panel() {
        let p = panel((800, 400));
        let c = p.corners_world();
        assert!((c[0] - Vec3::new(-0.5, 1.95, -2.0)).length() < 1e-5, "top-left {:?}", c[0]);
        assert!((c[2] - Vec3::new(0.5, 1.45, -2.0)).length() < 1e-5, "bottom-right {:?}", c[2]);
    }

    #[test]
    fn face_towards_camera() {
        let mut p = panel((800, 400));
        p.face_towards(Vec3::new(0.0, 1.7, 0.0)); // camera behind +Z
        assert!((p.normal() - Vec3::Z).length() < 1e-5);
        p.face_towards(Vec3::new(5.0, 0.0, -2.0)); // to the right, at another height
        assert!((p.normal() - Vec3::X).length() < 1e-5, "{:?}", p.normal());
        assert!(p.normal().y.abs() < 1e-6, "stays upright");
    }

    #[test]
    fn bbox_quad_extends_around_geometry() {
        let mut p = panel((800, 400));
        // 40 px shadow on every side.
        p.bbox_px = Rectangle::new((-40, -40).into(), (880, 480).into());
        let m = p.bbox_model_matrix();
        let tl = m.transform_point3(Vec3::new(-0.5, 0.5, 0.0));
        // Geometry top-left is (-0.5, 1.95); the bbox is 40 px = 0.05 m further out.
        assert!((tl - Vec3::new(-0.55, 2.0, -2.0)).length() < 1e-5, "{tl:?}");
        // Popup hanging below the window: bbox center moves down.
        p.bbox_px = Rectangle::new((0, 0).into(), (800, 600).into());
        let bottom = p.bbox_model_matrix().transform_point3(Vec3::new(0.0, -0.5, 0.0));
        assert!((bottom.y - (1.45 - 0.25)).abs() < 1e-5, "{bottom:?}");
    }
}
