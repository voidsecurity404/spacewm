//! The 2D desktop (Mod+D, or spacewm --render 2d): workspaces, tiling and a top bar, like
//! a tiling window manager.
//!
//! The windows are the same as in 3D: every window has a workspace (the one that was
//! current when it opened, in 2D or 3D) and a place in that workspace's tiling order.
//! In 2D only the current workspace is shown: its tiled windows share the area under
//! the bar (tiling.rs), floating ones keep their own rectangle, a fullscreen one covers
//! the whole screen. Windows of other workspaces are parked off-screen.
//!
//! A window can only have one size, so going to 2D remembers each window's 3D size and
//! going back gives it back.

pub mod bar;
pub mod tiling;

use std::collections::HashMap;

use smithay::{
    desktop::Window,
    utils::{Logical, Point, Rectangle, Size, SERIAL_COUNTER},
};

use crate::{cli::RenderMode, state::SpaceState, world::panel::PanelId};
use tiling::{Dir, Rect};

pub const WORKSPACES: u8 = 9;
/// Where windows of other workspaces wait (far outside every output).
const PARKED: Point<i32, Logical> = Point::new(-30000, -30000);

#[derive(Default)]
pub struct Desktop2d {
    /// 1..=9.
    pub current: u8,
    /// Each workspace's windows in tiling order (floating ones are listed too).
    order: HashMap<u8, Vec<PanelId>>,
    /// Floating windows and where they are.
    floating: HashMap<PanelId, Rect>,
    /// The window covering the screen, if any.
    pub fullscreen: Option<PanelId>,
    /// Window sizes in 3D, given back when leaving 2D.
    size_3d: HashMap<PanelId, (i32, i32)>,
    /// The window that had the keyboard on each workspace.
    last_focus: HashMap<u8, PanelId>,
    /// The layout area last laid out (re-layout when it changes: bars, monitor size).
    last_area: Option<Rect>,
    pub bar: bar::Bar,
    /// The focused window's rectangle, for its colored frame.
    pub border: Option<Rect>,
    /// A 1x1 pixel of the frame color, stretched into the frame's four lines.
    pub accent: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>,
}

impl Desktop2d {
    pub fn new() -> Self {
        Self { current: 1, ..Default::default() }
    }

    pub fn is_floating(&self, panel: PanelId) -> bool {
        self.floating.contains_key(&panel)
    }

    pub fn workspace_of(&self, panel: PanelId) -> Option<u8> {
        self.order.iter().find(|(_, v)| v.contains(&panel)).map(|(ws, _)| *ws)
    }

    /// Workspaces that have windows.
    pub fn occupied(&self) -> Vec<u8> {
        let mut v: Vec<u8> =
            self.order.iter().filter(|(_, w)| !w.is_empty()).map(|(k, _)| *k).collect();
        v.sort_unstable();
        v
    }

    fn current(&self) -> u8 {
        self.current.clamp(1, WORKSPACES)
    }
}

fn to_rect(r: Rectangle<i32, Logical>) -> Rect {
    Rect { x: r.loc.x, y: r.loc.y, w: r.size.w, h: r.size.h }
}

/// Move a window to `loc` and ask it to take `size`, only sending what changed.
fn place(state: &mut SpaceState, window: &Window, r: Rect) {
    let loc: Point<i32, Logical> = (r.x, r.y).into();
    if state.space.element_location(window) != Some(loc) {
        state.space.map_element(window.clone(), loc, false);
    }
    let size: Size<i32, Logical> = (r.w.max(1), r.h.max(1)).into();
    if let Some(toplevel) = window.toplevel() {
        let pending = toplevel.with_pending_state(|s| s.size);
        if pending != Some(size) {
            toplevel.with_pending_state(|s| s.size = Some(size));
            // Before its first configure the size goes out with that one.
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
        }
    } else if let Some(x11) = window.x11_surface() {
        let want = Rectangle::new(loc, size);
        if x11.geometry() != want {
            let _ = x11.configure(want);
        }
    }
}

impl SpaceState {
    /// A new window: it joins the current workspace (last in the tiling order).
    pub fn ws_add_window(&mut self, panel: PanelId) {
        let ws = self.desktop2d.current();
        self.desktop2d.order.entry(ws).or_default().push(panel);
        if self.render_mode == RenderMode::Flat2D {
            // It opens at a 3D size too when it is shown in 3D later.
            self.desktop2d.size_3d.insert(panel, crate::wayland::xdg_shell::DEFAULT_WINDOW_SIZE);
            self.apply_layout();
        }
        self.desktop2d.bar.dirty = true;
    }

    /// A window closed.
    pub fn ws_remove_window(&mut self, panel: PanelId) {
        for list in self.desktop2d.order.values_mut() {
            list.retain(|p| *p != panel);
        }
        self.desktop2d.floating.remove(&panel);
        self.desktop2d.size_3d.remove(&panel);
        self.desktop2d.last_focus.retain(|_, p| *p != panel);
        if self.desktop2d.fullscreen == Some(panel) {
            self.desktop2d.fullscreen = None;
        }
        self.desktop2d.bar.dirty = true;
        if self.render_mode == RenderMode::Flat2D {
            self.apply_layout();
            self.focus_workspace_window();
        }
    }

    /// The area windows tile in: the output minus bars (ours and layer-shell ones).
    fn layout_area(&self) -> Option<Rect> {
        let output = self.space.outputs().next()?.clone();
        let geo = self.space.output_geometry(&output)?;
        let zone = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
        let top = bar::HEIGHT;
        Some(Rect {
            x: geo.loc.x + zone.loc.x,
            y: geo.loc.y + zone.loc.y + top,
            w: zone.size.w,
            h: (zone.size.h - top).max(1),
        })
    }

    /// Put every window where it belongs on the 2D desktop.
    pub fn apply_layout(&mut self) {
        if self.render_mode != RenderMode::Flat2D {
            return;
        }
        let Some(area) = self.layout_area() else { return };
        let full = self.primary_output_geometry().map(to_rect).unwrap_or(area);
        self.desktop2d.last_area = Some(area);
        let ws = self.desktop2d.current();
        let windows: HashMap<PanelId, Window> =
            self.world.panels().iter().map(|p| (p.id, p.window.clone())).collect();
        // Windows we never heard of (none should exist): put them on this workspace.
        for id in windows.keys() {
            if self.desktop2d.workspace_of(*id).is_none() {
                self.desktop2d.order.entry(ws).or_default().push(*id);
            }
        }
        let shown: Vec<PanelId> = self.desktop2d.order.get(&ws).cloned().unwrap_or_default();
        // Others wait off-screen.
        for (id, w) in &windows {
            if !shown.contains(id) && self.space.element_location(w) != Some(PARKED) {
                self.space.map_element(w.clone(), PARKED, false);
            }
        }
        let fullscreen = self.desktop2d.fullscreen.filter(|f| shown.contains(f));
        let tiled: Vec<PanelId> = shown
            .iter()
            .copied()
            .filter(|id| !self.desktop2d.floating.contains_key(id) && Some(*id) != fullscreen)
            .collect();
        // Stacking: tiled first, floating above them, fullscreen on top.
        for (id, r) in tiled.iter().zip(tiling::tile(area, tiled.len())) {
            if let Some(w) = windows.get(id) {
                place(self, w, r);
            }
        }
        for id in &shown {
            if let (Some(r), Some(w)) = (self.desktop2d.floating.get(id).copied(), windows.get(id))
            {
                if Some(*id) != fullscreen {
                    place(self, w, r);
                }
            }
        }
        if let Some(w) = fullscreen.and_then(|f| windows.get(&f)) {
            place(self, w, full);
        }
        self.desktop2d.bar.dirty = true;
        self.queue_redraw();
    }

    /// Before each frame in 2D: re-lay out when the area changed (a bar appeared, the
    /// nested window was resized).
    pub fn refresh_2d(&mut self) {
        if self.render_mode != RenderMode::Flat2D {
            return;
        }
        if self.layout_area() != self.desktop2d.last_area {
            self.apply_layout();
        }
        self.refresh_bar();
        // The frame goes around the window with the keyboard (none over fullscreen).
        let border = self
            .focused_panel()
            .filter(|p| self.desktop2d.fullscreen != Some(*p))
            .filter(|p| self.desktop2d.workspace_of(*p) == Some(self.desktop2d.current()))
            .and_then(|p| self.rect_2d(p));
        if border != self.desktop2d.border {
            self.desktop2d.border = border;
            self.queue_redraw();
        }
        if self.desktop2d.accent.is_none() {
            let px = [64u8, 115, 199, 255]; // R, G, B, A
            self.desktop2d.accent =
                Some(smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
                    &px,
                    smithay::backend::allocator::Fourcc::Abgr8888,
                    (1, 1),
                    1,
                    smithay::utils::Transform::Normal,
                    None,
                ));
        }
    }

    /// The panel of the window with the keyboard.
    fn focused_panel(&self) -> Option<PanelId> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        let crate::focus::KeyboardFocusTarget::Window(w) = focus else { return None };
        self.world.panel_for_window(&w).map(|p| p.id)
    }

    fn window_of(&self, panel: PanelId) -> Option<Window> {
        self.world.panels().iter().find(|p| p.id == panel).map(|p| p.window.clone())
    }

    /// Give the keyboard to the window that last had it on this workspace (or the first).
    fn focus_workspace_window(&mut self) {
        let ws = self.desktop2d.current();
        let shown = self.desktop2d.order.get(&ws).cloned().unwrap_or_default();
        let pick = self
            .desktop2d
            .last_focus
            .get(&ws)
            .copied()
            .filter(|p| shown.contains(p))
            .or_else(|| shown.first().copied());
        let window = pick.and_then(|p| self.window_of(p));
        self.focus_window(window.as_ref(), SERIAL_COUNTER.next_serial());
    }

    /// Remember which window has the keyboard on the current workspace.
    pub fn note_2d_focus(&mut self) {
        if let Some(p) = self.focused_panel() {
            let ws = self.desktop2d.current();
            if self.desktop2d.order.get(&ws).is_some_and(|v| v.contains(&p)) {
                self.desktop2d.last_focus.insert(ws, p);
            }
        }
    }

    /// Mod+1..9.
    pub fn switch_workspace(&mut self, n: u8) {
        let n = n.clamp(1, WORKSPACES);
        if n == self.desktop2d.current() && self.desktop2d.current != 0 {
            return;
        }
        self.note_2d_focus();
        tracing::info!(workspace = n, "workspace");
        self.desktop2d.current = n;
        self.desktop2d.bar.dirty = true;
        if self.render_mode == RenderMode::Flat2D {
            self.apply_layout();
            self.focus_workspace_window();
        } else {
            // In 3D it decides where new windows go in 2D.
            self.show_info(format!("Workspace {n}: new windows go here (Mod+D shows it)"));
        }
    }

    /// Put a window on workspace `n` quietly (restoring a saved session).
    pub fn ws_assign(&mut self, panel: PanelId, n: u8) {
        for list in self.desktop2d.order.values_mut() {
            list.retain(|p| *p != panel);
        }
        self.desktop2d.order.entry(n).or_default().push(panel);
        self.desktop2d.bar.dirty = true;
        self.apply_layout();
    }

    /// Mod+Shift+1..9: the focused window (3D: the one looked at) to workspace `n`.
    pub fn move_to_workspace(&mut self, n: u8) {
        let n = n.clamp(1, WORKSPACES);
        let panel = match self.render_mode {
            RenderMode::Flat2D => self.focused_panel(),
            RenderMode::Scene3D => crate::input::world_ops::target_panel(self),
        };
        let Some(panel) = panel else { return };
        if self.desktop2d.workspace_of(panel) == Some(n) {
            return;
        }
        for list in self.desktop2d.order.values_mut() {
            list.retain(|p| *p != panel);
        }
        self.desktop2d.order.entry(n).or_default().push(panel);
        tracing::info!(panel, workspace = n, "window moved to workspace");
        self.desktop2d.bar.dirty = true;
        if self.render_mode == RenderMode::Flat2D {
            self.apply_layout();
            self.focus_workspace_window();
        } else {
            self.show_info(format!("Window moved to workspace {n}"));
        }
    }

    /// Mod+V: the focused window floats (centered) or joins the tiling again.
    pub fn toggle_float(&mut self) {
        if self.render_mode != RenderMode::Flat2D {
            return;
        }
        let Some(panel) = self.focused_panel() else { return };
        if self.desktop2d.floating.remove(&panel).is_none() {
            let area = self.layout_area().unwrap_or(Rect { x: 0, y: 0, w: 1280, h: 720 });
            let (w, h) = (area.w * 3 / 5, area.h * 3 / 5);
            let r = Rect { x: area.x + (area.w - w) / 2, y: area.y + (area.h - h) / 2, w, h };
            self.desktop2d.floating.insert(panel, r);
        }
        self.apply_layout();
    }

    /// A Mod+drag moved/resized a window: it floats there.
    pub fn set_floating_rect(&mut self, panel: PanelId, r: Rect) {
        self.desktop2d.floating.insert(panel, r);
        if let Some(w) = self.window_of(panel) {
            place(self, &w, r);
        }
        self.queue_redraw();
    }

    /// The rectangle a window has now on the 2D desktop.
    pub fn rect_2d(&self, panel: PanelId) -> Option<Rect> {
        let w = self.window_of(panel)?;
        let loc = self.space.element_location(&w)?;
        let size = w.geometry().size;
        Some(Rect { x: loc.x, y: loc.y, w: size.w, h: size.h })
    }

    /// Mod+F in 2D (and apps asking): the window covers the screen, or not.
    pub fn toggle_fullscreen_2d(&mut self, panel: Option<PanelId>) {
        let Some(panel) = panel.or_else(|| self.focused_panel()) else { return };
        let on = self.desktop2d.fullscreen != Some(panel);
        self.set_fullscreen_2d(panel, on);
    }

    pub fn set_fullscreen_2d(&mut self, panel: PanelId, on: bool) {
        let Some(window) = self.window_of(panel) else { return };
        if on {
            if let Some(old) = self.desktop2d.fullscreen.filter(|o| *o != panel) {
                if let Some(w) = self.window_of(old) {
                    crate::input::fullscreen::set_fullscreen_state(&w, None);
                }
            }
            self.desktop2d.fullscreen = Some(panel);
            let full = self.primary_output_geometry();
            crate::input::fullscreen::set_fullscreen_state(&window, full);
        } else if self.desktop2d.fullscreen == Some(panel) {
            self.desktop2d.fullscreen = None;
            crate::input::fullscreen::set_fullscreen_state(&window, None);
        }
        tracing::info!(panel, on, "2d fullscreen");
        self.apply_layout();
    }

    /// Mod+arrows: the keyboard to the neighbor in that direction.
    pub fn focus_dir(&mut self, dir: Dir) {
        let Some((idx, panels, rects)) = self.visible_rects() else { return };
        if let Some(j) = tiling::neighbor(rects[idx], &rects, dir) {
            if let Some(w) = self.window_of(panels[j]) {
                self.focus_window(Some(&w), SERIAL_COUNTER.next_serial());
                self.note_2d_focus();
                self.desktop2d.bar.dirty = true;
            }
        }
    }

    /// Mod+Shift+arrows: swap the focused window with its neighbor in the tiling order.
    pub fn move_dir(&mut self, dir: Dir) {
        let Some((idx, panels, rects)) = self.visible_rects() else { return };
        let Some(j) = tiling::neighbor(rects[idx], &rects, dir) else { return };
        let (a, b) = (panels[idx], panels[j]);
        let ws = self.desktop2d.current();
        if let Some(list) = self.desktop2d.order.get_mut(&ws) {
            if let (Some(i), Some(k)) =
                (list.iter().position(|p| *p == a), list.iter().position(|p| *p == b))
            {
                list.swap(i, k);
            }
        }
        self.apply_layout();
    }

    /// The focused window's index among the shown ones, with all their rectangles.
    fn visible_rects(&self) -> Option<(usize, Vec<PanelId>, Vec<Rect>)> {
        if self.render_mode != RenderMode::Flat2D {
            return None;
        }
        let focused = self.focused_panel()?;
        let ws = self.desktop2d.current();
        let panels: Vec<PanelId> = self.desktop2d.order.get(&ws)?.clone();
        let rects: Vec<Rect> = panels.iter().filter_map(|p| self.rect_2d(*p)).collect();
        if rects.len() != panels.len() {
            return None;
        }
        Some((panels.iter().position(|p| *p == focused)?, panels, rects))
    }

    /// Going into 2D: remember 3D sizes, lay out.
    pub fn enter_2d(&mut self) {
        if self.desktop2d.current == 0 {
            self.desktop2d.current = 1;
        }
        self.desktop2d.size_3d = self
            .world
            .panels()
            .iter()
            .map(|p| {
                let s = p.window.geometry().size;
                let size = if s.w > 0 && s.h > 0 { (s.w, s.h) } else { p.size_px };
                (p.id, size)
            })
            .collect();
        self.desktop2d.last_area = None;
        self.apply_layout();
        self.focus_workspace_window();
    }

    /// Leaving 2D: every window gets its 3D size back, nothing is fullscreen.
    pub fn leave_2d(&mut self) {
        self.note_2d_focus();
        if let Some(f) = self.desktop2d.fullscreen.take() {
            if let Some(w) = self.window_of(f) {
                crate::input::fullscreen::set_fullscreen_state(&w, None);
            }
        }
        let sizes: Vec<(PanelId, (i32, i32))> = self.desktop2d.size_3d.drain().collect();
        for (panel, size) in sizes {
            if let Some(w) = self.window_of(panel) {
                crate::input::world_ops::resize_window(&w, size);
            }
        }
    }
}
