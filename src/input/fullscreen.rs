//! Fullscreen (Mod+F, or an app asking for it: F11 in a browser, a video player).
//!
//! The panel's app is told the screen size and the `fullscreen` state (X11 apps get
//! _NET_WM_STATE_FULLSCREEN), and spacewm stops drawing the 3D world: the window is drawn
//! flat over the whole output, one window pixel per screen pixel, like on any desktop.
//! Input works as in Window mode; the mouse cursor is an ordinary 2D cursor.
//!
//! Leaving: Mod+F again (stays in the window), Mod+Escape (back to walking), or the app
//! asks. The window gets its old size back and is a normal panel again.

use smithay::{desktop::Window, reexports::wayland_protocols::xdg::shell::server::xdg_toplevel};

use super::mode::{set_mode, InputMode};
use crate::{state::SpaceState, world::panel::PanelId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fullscreen {
    pub panel: PanelId,
    /// The window size before, restored when leaving.
    pub restore_size: (i32, i32),
}

impl SpaceState {
    pub fn fullscreen_panel(&self) -> Option<PanelId> {
        self.fullscreen.map(|f| f.panel)
    }

    pub fn enter_fullscreen(&mut self, panel: PanelId) {
        if self.render_mode != crate::cli::RenderMode::Scene3D
            || self.fullscreen_panel() == Some(panel)
        {
            return;
        }
        self.exit_fullscreen();
        let Some(window) =
            self.world.panels().iter().find(|p| p.id == panel).map(|p| p.window.clone())
        else {
            return;
        };
        let Some(output) = self.primary_output_geometry() else { return };
        let size = window.geometry().size;
        tracing::info!(panel, w = output.size.w, h = output.size.h, "panel fullscreen");
        self.close_inventory();
        self.grab = None;
        set_fullscreen_state(&window, Some(output));
        self.fullscreen = Some(Fullscreen { panel, restore_size: (size.w, size.h) });
        if self.mode != (InputMode::Window { panel }) {
            set_mode(self, InputMode::Window { panel }, None);
        }
        self.view.dirty = true;
        self.queue_redraw();
    }

    /// Back to a normal panel (no-op when nothing is fullscreen).
    pub fn exit_fullscreen(&mut self) {
        let Some(fs) = self.fullscreen.take() else { return };
        tracing::info!(panel = fs.panel, "panel leaves fullscreen");
        if let Some(window) =
            self.world.panels().iter().find(|p| p.id == fs.panel).map(|p| p.window.clone())
        {
            set_fullscreen_state(&window, None);
            super::world_ops::resize_window(&window, fs.restore_size);
        }
        // Textures of panels were not updated while the world was hidden.
        for p in self.world.panels_mut() {
            p.content_dirty = true;
        }
        self.view.dirty = true;
        self.queue_redraw();
    }

    /// Mod+F: the target panel goes fullscreen, or the fullscreen one comes back.
    pub fn toggle_fullscreen(&mut self, target: Option<PanelId>) {
        if self.fullscreen.is_some() {
            self.exit_fullscreen();
        } else if let Some(panel) = target {
            self.enter_fullscreen(panel);
        }
    }
}

/// Tell the app it is (or no longer is) fullscreen. `Some(rect)`: fullscreen on that
/// output area. Leaving, the caller sets the size and sends the configure.
fn set_fullscreen_state(
    window: &Window,
    output: Option<smithay::utils::Rectangle<i32, smithay::utils::Logical>>,
) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|s| match output {
            Some(rect) => {
                s.states.set(xdg_toplevel::State::Fullscreen);
                s.size = Some(rect.size);
            }
            None => {
                s.states.unset(xdg_toplevel::State::Fullscreen);
            }
        });
        // Leaving, the caller's resize sends this together with the old size: one
        // configure, so the app doesn't draw a frame at the fullscreen size first.
        if output.is_some() {
            toplevel.send_pending_configure();
        }
    } else if let Some(x11) = window.x11_surface() {
        if let Err(e) = x11.set_fullscreen(output.is_some()) {
            tracing::debug!("X11 fullscreen state: {e}");
        }
        if let Some(rect) = output {
            if let Err(e) = x11.configure(rect) {
                tracing::debug!("X11 fullscreen configure: {e}");
            }
        }
    }
}
