//! The window switcher (Mod+Tab / Mod+grave): every open window in a list, the most
//! recently used first, with its icon, name, title and workspace. Hold Mod and press
//! the key again to go down the list; let go of Mod to go to the chosen window. Arrows,
//! Enter and a click work too; Escape cancels.
//!
//! Going to a window: in 3D it comes in front of you (like the dock); on the 2D desktop
//! its workspace is shown and it gets the keyboard.

use smithay::{
    input::{keyboard::Keysym, pointer::CursorImageStatus},
    utils::{Logical, Point},
};

use crate::{
    cli::RenderMode,
    inventory::{draw::Canvas, Image},
    state::SpaceState,
    world::panel::PanelId,
};

/// Layout in logical px.
const WIDTH: i32 = 560;
const PAD: i32 = 12;
const ROW_H: i32 = 46;
const ICON: i32 = 32;
/// Rows shown at once (the list scrolls with the selection).
const VISIBLE: usize = 9;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub panel: PanelId,
    pub name: String,
    pub title: String,
    pub workspace: u8,
}

pub struct Switcher {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub image: Option<Image>,
    drawn: Option<(usize, i32)>,
}

/// The first row shown so that `selected` is visible.
fn first_row(len: usize, selected: usize) -> usize {
    if len <= VISIBLE {
        0
    } else {
        selected.saturating_sub(VISIBLE - 1).min(len - VISIBLE)
    }
}

pub fn box_size(len: usize) -> (i32, i32) {
    (WIDTH, 2 * PAD + len.clamp(1, VISIBLE) as i32 * ROW_H)
}

pub fn box_origin(len: usize, output: (i32, i32)) -> (i32, i32) {
    let (w, h) = box_size(len);
    ((output.0 - w) / 2, ((output.1 - h) / 2).max(0))
}

/// The entry at a point relative to the box (given the scroll).
pub fn entry_at(len: usize, selected: usize, p: (f64, f64)) -> Option<usize> {
    if p.0 < 0.0 || p.0 >= WIDTH as f64 || p.1 < PAD as f64 {
        return None;
    }
    let row = ((p.1 - PAD as f64) / ROW_H as f64) as usize;
    let i = first_row(len, selected) + row;
    (row < VISIBLE && i < len).then_some(i)
}

impl SpaceState {
    /// Windows, most recently used first (then the rest in opening order).
    fn switcher_entries(&self) -> Vec<Entry> {
        let mut panels: Vec<PanelId> = self.mru.clone();
        for p in self.world.panels() {
            if !panels.contains(&p.id) {
                panels.push(p.id);
            }
        }
        panels
            .into_iter()
            .filter_map(|id| {
                let p = self.world.panels().iter().find(|p| p.id == id)?;
                let (app, title) = crate::state::window_names(&p.window);
                let app = app.or_else(|| p.names.0.clone()).unwrap_or_else(|| "window".into());
                let name = self.app_display_name(&app);
                Some(Entry {
                    panel: id,
                    name,
                    title: title.unwrap_or_default(),
                    workspace: self.desktop2d.workspace_of(id).unwrap_or(1),
                })
            })
            .collect()
    }

    /// Mod+Tab: open the list on the window before this one; pressed again: next.
    pub fn switcher_next(&mut self, back: bool) {
        if let Some(s) = self.switcher.as_mut() {
            let n = s.entries.len().max(1);
            s.selected = if back { (s.selected + n - 1) % n } else { (s.selected + 1) % n };
            self.queue_redraw();
            return;
        }
        // Names and icons come from the apps' .desktop files.
        self.ensure_entries();
        let entries = self.switcher_entries();
        if entries.is_empty() {
            return;
        }
        // The first entry is the window in use: start on the one before it.
        let selected = if entries.len() > 1 { 1 } else { 0 };
        tracing::info!(windows = entries.len(), "window switcher");
        self.close_menu();
        self.set_dock_cursor(false);
        self.view.controls.clear();
        if let Some(geo) = self.primary_output_geometry() {
            self.pointer_location = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
        }
        self.cursor_status = CursorImageStatus::default_named();
        self.switcher = Some(Switcher { entries, selected, image: None, drawn: None });
        self.queue_redraw();
    }

    pub fn switcher_cancel(&mut self) {
        if self.switcher.take().is_some() {
            self.view.dirty = true;
            self.queue_redraw();
        }
    }

    /// Go to the chosen window and close the list.
    pub fn switcher_commit(&mut self) {
        let Some(s) = self.switcher.take() else { return };
        self.view.dirty = true;
        let Some(e) = s.entries.get(s.selected).cloned() else { return };
        tracing::info!(panel = e.panel, "switcher: go to window");
        match self.render_mode {
            RenderMode::Scene3D => self.summon_panel(e.panel),
            RenderMode::Flat2D => {
                self.switch_workspace(e.workspace);
                if let Some(w) =
                    self.world.panels().iter().find(|p| p.id == e.panel).map(|p| p.window.clone())
                {
                    self.focus_window(Some(&w), smithay::utils::SERIAL_COUNTER.next_serial());
                    self.note_2d_focus();
                }
            }
        }
        self.queue_redraw();
    }

    pub fn switcher_key(&mut self, sym: Keysym) {
        match sym {
            Keysym::Escape => self.switcher_cancel(),
            Keysym::Return | Keysym::KP_Enter => self.switcher_commit(),
            Keysym::Down | Keysym::Right | Keysym::Tab | Keysym::grave => self.switcher_next(false),
            Keysym::Up | Keysym::Left | Keysym::ISO_Left_Tab => self.switcher_next(true),
            _ => {}
        }
    }

    pub fn switcher_pointer(&mut self, pos: Point<f64, Logical>) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let max: Point<f64, Logical> = Point::from((
            (geo.loc.x + geo.size.w) as f64 - 1.0,
            (geo.loc.y + geo.size.h) as f64 - 1.0,
        ));
        self.pointer_location = Point::from((
            pos.x.clamp(geo.loc.x as f64, max.x),
            pos.y.clamp(geo.loc.y as f64, max.y),
        ));
        if let Some(s) = self.switcher.as_mut() {
            let o = box_origin(s.entries.len(), (geo.size.w, geo.size.h));
            let local =
                self.pointer_location - geo.loc.to_f64() - Point::from((o.0 as f64, o.1 as f64));
            if let Some(i) = entry_at(s.entries.len(), s.selected, (local.x, local.y)) {
                s.selected = i;
            }
        }
        self.queue_redraw();
    }

    /// A click: on an entry goes there, elsewhere cancels.
    pub fn switcher_click(&mut self) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let Some(s) = self.switcher.as_ref() else { return };
        let o = box_origin(s.entries.len(), (geo.size.w, geo.size.h));
        let local =
            self.pointer_location - geo.loc.to_f64() - Point::from((o.0 as f64, o.1 as f64));
        match entry_at(s.entries.len(), s.selected, (local.x, local.y)) {
            Some(_) => self.switcher_commit(),
            None => self.switcher_cancel(),
        }
    }

    pub fn refresh_switcher(&mut self) {
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        let Some(s) = self.switcher.as_ref() else { return };
        if s.drawn == Some((s.selected, scale)) {
            return;
        }
        let entries = s.entries.clone();
        let selected = s.selected;
        let icon_px = (ICON * scale) as u32;
        let apps: Vec<Option<String>> = entries
            .iter()
            .map(|e| {
                self.world.panels().iter().find(|p| p.id == e.panel).and_then(|p| {
                    crate::state::window_names(&p.window).0.or_else(|| p.names.0.clone())
                })
            })
            .collect();
        let icons: Vec<Option<Vec<u8>>> =
            apps.iter().map(|a| a.as_ref().and_then(|a| self.app_icon(a, icon_px))).collect();
        let two_d = self.render_mode == RenderMode::Flat2D;
        let image = draw(&entries, &icons, selected, two_d, scale);
        if let Some(s) = self.switcher.as_mut() {
            s.image = image;
            s.drawn = Some((selected, scale));
        }
    }
}

fn draw(
    entries: &[Entry],
    icons: &[Option<Vec<u8>>],
    selected: usize,
    two_d: bool,
    scale: i32,
) -> Option<Image> {
    let s = scale;
    let (w, h) = box_size(entries.len());
    let mut c = Canvas::new(w * s, h * s);
    c.fill(0, 0, w * s, h * s, [0.07, 0.08, 0.10, 0.95]);
    c.frame(0, 0, w * s, h * s, s, [0.32, 0.36, 0.42, 1.0]);
    let first = first_row(entries.len(), selected);
    for (row, i) in (first..entries.len()).take(VISIBLE).enumerate() {
        let e = &entries[i];
        let y = PAD + row as i32 * ROW_H;
        if i == selected {
            c.fill((PAD - 4) * s, y * s, (w - 2 * PAD + 8) * s, ROW_H * s, [0.20, 0.40, 0.70, 1.0]);
        }
        let (ix, iy) = ((PAD + 6) * s, (y + (ROW_H - ICON) / 2) * s);
        match icons.get(i).and_then(|i| i.as_ref()) {
            Some(px) => c.blit(px, ICON * s, ICON * s, ix, iy),
            None => {
                c.fill(ix, iy, ICON * s, ICON * s, [0.30, 0.45, 0.70, 1.0]);
                let letter: String =
                    e.name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
                c.text(&letter, 18.0, [1.0; 4], ix + 9 * s, iy + 4 * s, s);
            }
        }
        let tx = (PAD + 6 + ICON + 12) * s;
        c.text(&e.name, 15.0, [0.95, 0.96, 0.98, 1.0], tx, (y + 5) * s, s);
        let title: String = e.title.chars().take(56).collect();
        if !title.is_empty() && title != e.name {
            c.text(&title, 12.0, [0.62, 0.66, 0.72, 1.0], tx, (y + 25) * s, s);
        }
        if two_d {
            let tag = format!("{}", e.workspace);
            let tw = Canvas::text_width(&tag, 13.0, s);
            c.fill((w - PAD - 30) * s, (y + 12) * s, 24 * s, 22 * s, [0.22, 0.25, 0.30, 1.0]);
            c.text(
                &tag,
                13.0,
                [0.90, 0.92, 0.95, 1.0],
                (w - PAD - 18) * s - tw / 2,
                (y + 14) * s,
                s,
            );
        }
    }
    let buffer = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
        &c.rgba,
        smithay::backend::allocator::Fourcc::Abgr8888,
        (c.width, c.height),
        s,
        smithay::utils::Transform::Normal,
        None,
    );
    Some((buffer, (w, h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolls_with_the_selection() {
        assert_eq!(first_row(5, 4), 0);
        assert_eq!(first_row(20, 3), 0);
        assert_eq!(first_row(20, 12), 12 - (VISIBLE - 1));
        assert_eq!(first_row(20, 19), 20 - VISIBLE);
    }

    #[test]
    fn hit_test() {
        let y = (PAD + ROW_H + 5) as f64;
        assert_eq!(entry_at(5, 0, (20.0, y)), Some(1));
        assert_eq!(entry_at(5, 0, (20.0, 2.0)), None);
        assert_eq!(entry_at(1, 0, (20.0, y)), None, "past the last entry");
        // Scrolled: row 0 is entry `first_row`.
        assert_eq!(entry_at(20, 19, (20.0, PAD as f64 + 1.0)), Some(20 - VISIBLE));
    }

    #[test]
    fn renders() {
        let e =
            vec![Entry { panel: 1, name: "Foot".into(), title: "~/spacewm".into(), workspace: 2 }];
        let (_, size) = draw(&e, &[None], 0, true, 1).expect("font");
        assert_eq!(size, box_size(1));
    }
}
