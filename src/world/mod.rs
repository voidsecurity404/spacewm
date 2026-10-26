//! The world: panels (Stage 3) and, from Stage 5, the voxel terrain.
//!
//! In 3D mode the world is the layout of windows. The 2D `Space` in `SpaceState` still
//! holds the same windows for `--render 2d`, output tracking (wl_surface.enter) and
//! frame callbacks; the two simply describe the same windows in different ways.

pub mod block;
pub mod body;
pub mod character;
pub mod chunk;
pub mod layout;
pub mod map;
pub mod mesh;
pub mod panel;
pub mod player;
pub mod scenery;
pub mod terrain;
pub mod voxels;

use glam::Vec3;
use smithay::wayland::seat::WaylandFocus;
use smithay::{desktop::Window, reexports::wayland_server::protocol::wl_surface::WlSurface};

use crate::render::camera::Camera;
use panel::{Panel, PanelId, DEFAULT_PIXELS_PER_UNIT};

/// A new panel is moved aside if another panel center is closer than this.
const MIN_SPAWN_SEPARATION: f32 = 0.3;

pub struct World<W = Window> {
    panels: Vec<Panel<W>>,
    /// Scale of new panels (config `panels.default_pixels_per_unit`).
    pub default_ppu: f32,
}

impl<W> Default for World<W> {
    fn default() -> Self {
        Self { panels: Vec::new(), default_ppu: DEFAULT_PIXELS_PER_UNIT }
    }
}

/// The distance at which a panel with `ppu` pixels per metre shows its texels 1:1 on an
/// output `output_height` px tall with vertical FOV `fov_y_deg`.
///
/// On screen, one metre at distance d spans H / (2·d·tan(fov/2)) pixels. Setting that
/// equal to ppu gives d = H / (2·ppu·tan(fov/2)). Closer is blurry-big, farther shrinks
/// text below readability — so new windows spawn exactly there.
pub fn pixel_perfect_distance(output_height: i32, ppu: f32, fov_y_deg: f32) -> f32 {
    let half = (fov_y_deg.to_radians() / 2.0).tan();
    (output_height.max(1) as f32 / (2.0 * ppu * half)).clamp(0.5, 3.0)
}

/// How far to put a new panel of `size_m` metres: pixel-perfect if it fits, otherwise
/// far enough back that it covers at most 85% of the view (small outputs).
pub fn spawn_distance(
    size_m: glam::Vec2,
    output_size: (i32, i32),
    ppu: f32,
    fov_y_deg: f32,
) -> f32 {
    let half = (fov_y_deg.to_radians() / 2.0).tan();
    let aspect = output_size.0.max(1) as f32 / output_size.1.max(1) as f32;
    // The view at distance d is 2·d·tan(fov/2) tall and `aspect` times that wide.
    let fit_h = size_m.y / (0.85 * 2.0 * half);
    let fit_w = size_m.x / (0.85 * 2.0 * half * aspect);
    pixel_perfect_distance(output_size.1, ppu, fov_y_deg).max(fit_h).max(fit_w).clamp(0.5, 3.0)
}

impl<W: PartialEq> World<W> {
    /// Place a new panel `distance` metres in front of the camera along its view direction
    /// (so it appears centered even when looking slightly down), upright and facing the
    /// camera. Panels never spawn inside each other: if the spot is taken, slide right
    /// by 1.1 panel widths until it is free.
    ///
    /// `id` is the window's id (`state::window_id`): a monotonic counter, never reused, so
    /// IPC and panels share one numbering.
    pub fn add_window(
        &mut self,
        id: PanelId,
        window: W,
        size_px: (i32, i32),
        camera: &Camera,
        distance: f32,
    ) -> PanelId {
        let mut panel = Panel::new(id, window, size_px);
        panel.pixels_per_unit = self.default_ppu;

        let right = camera.forward_flat().cross(Vec3::Y);
        let mut pos = camera.position + camera.forward() * distance;
        let step = right * panel.world_size().x * 1.1;
        for _ in 0..64 {
            let taken = self.panels.iter().any(|p| p.position.distance(pos) < MIN_SPAWN_SEPARATION);
            if !taken {
                break;
            }
            pos += step;
        }
        panel.position = pos;
        panel.face_towards(camera.position);
        self.panels.push(panel);
        id
    }

    pub fn remove_window(&mut self, window: &W) -> Option<Panel<W>> {
        let index = self.panels.iter().position(|p| &p.window == window)?;
        Some(self.panels.remove(index))
    }

    pub fn panels(&self) -> &[Panel<W>] {
        &self.panels
    }

    pub fn panels_mut(&mut self) -> &mut [Panel<W>] {
        &mut self.panels
    }

    pub fn panel_for_window(&self, window: &W) -> Option<&Panel<W>> {
        self.panels.iter().find(|p| &p.window == window)
    }

    #[allow(dead_code)] // used by IPC focus/move requests (Stage 7.3)
    pub fn panel(&self, id: PanelId) -> Option<&Panel<W>> {
        self.panels.iter().find(|p| p.id == id)
    }
}

impl World<Window> {
    /// The panel whose window's surface tree contains `root` (a toplevel surface).
    pub fn panel_for_surface_mut(&mut self, root: &WlSurface) -> Option<&mut Panel> {
        self.panels.iter_mut().find(|p| {
            p.window.wl_surface().as_deref() == Some(root)
                || p.x11_children.iter().any(|c| c.surface.wl_surface().as_ref() == Some(root))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        Camera { position: Vec3::new(0.0, 1.7, 0.0), yaw: 0.0, pitch: -0.3, ..Camera::default() }
    }

    #[test]
    fn spawns_in_front_at_eye_height_facing_camera() {
        let mut world = World::<u32>::default();
        let id = world.add_window(1, 7, (1000, 700), &camera(), 2.0);
        let p = world.panel(id).expect("panel");
        let expected = camera().position + camera().forward() * 2.0;
        assert!((p.position - expected).length() < 1e-5, "{:?}", p.position);
        assert!((p.normal() - Vec3::Z).length() < 1e-5, "faces the camera");
    }

    #[test]
    fn second_window_slides_right() {
        let mut world = World::<u32>::default();
        world.add_window(1, 1, (1000, 700), &camera(), 2.0);
        let id = world.add_window(2, 2, (1000, 700), &camera(), 2.0);
        let p = world.panel(id).expect("panel");
        // 1.25 m wide × 1.1 to the right of the first.
        assert!((p.position.x - 1.375).abs() < 1e-4, "{:?}", p.position);
        assert!(world.panels().len() == 2);
        let third = world.add_window(3, 3, (1000, 700), &camera(), 2.0);
        assert!((world.panel(third).expect("panel").position.x - 2.75).abs() < 1e-4);
    }

    #[test]
    fn remove_and_lookup() {
        let mut world = World::<u32>::default();
        world.add_window(1, 1, (100, 100), &camera(), 2.0);
        world.add_window(2, 2, (100, 100), &camera(), 2.0);
        assert!(world.remove_window(&1).is_some());
        assert!(world.panel_for_window(&1).is_none());
        assert!(world.panel_for_window(&2).is_some());
    }

    #[test]
    fn spawn_distance_fits_small_outputs() {
        let size = glam::Vec2::new(1.25, 0.875);
        // 1080p: the 1:1 distance (0.88 m) already fits.
        let d = spawn_distance(size, (1920, 1080), 800.0, 75.0);
        assert!((d - 0.88).abs() < 0.01, "{d}");
        // 720p: 1:1 would be 0.59 m and overflow; back off until 85% of the height.
        let d = spawn_distance(size, (1280, 720), 800.0, 75.0);
        let visible_h = 2.0 * d * (37.5f32).to_radians().tan();
        assert!((size.y / visible_h - 0.85).abs() < 0.01, "{d}");
    }

    #[test]
    fn pixel_perfect_distance_math() {
        // 1080p, 800 px/m, 75° FOV → about 0.88 m.
        let d = pixel_perfect_distance(1080, 800.0, 75.0);
        assert!((d - 0.88).abs() < 0.01, "{d}");
        // Check the definition: at that distance 1 m spans ppu pixels.
        let px_per_m = 1080.0 / (2.0 * d * (37.5f32).to_radians().tan());
        assert!((px_per_m - 800.0).abs() < 1.0);
        assert_eq!(pixel_perfect_distance(10_000, 100.0, 75.0), 3.0, "clamped");
    }
}
