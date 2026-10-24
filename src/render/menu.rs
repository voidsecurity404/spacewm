//! The window menu: right click on a window (or Mod+right click while using one) lists
//! what can be done with it, each with its key next to it, so nothing has to be
//! remembered and the keys are learned along the way.
//!
//! While it is open the mouse moves an ordinary cursor (the view stays still); a click
//! on an entry does it, a click elsewhere or Escape closes the menu; arrow keys and
//! Enter work too.

use smithay::{
    input::pointer::CursorImageStatus,
    utils::{Logical, Point},
};

use super::text::{self, Line, Style};
use crate::{
    input::{
        mode::{set_mode, InputMode},
        world_ops,
    },
    inventory::{draw::Canvas, Image},
    state::SpaceState,
    world::panel::PanelId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Use,
    Fullscreen,
    Move,
    Bigger,
    Smaller,
    MoreRows,
    FewerRows,
    Rotate,
    FaceMe,
    Hide,
    Close,
}

pub const ITEMS: [Item; 11] = [
    Item::Use,
    Item::Fullscreen,
    Item::Move,
    Item::Bigger,
    Item::Smaller,
    Item::MoreRows,
    Item::FewerRows,
    Item::Rotate,
    Item::FaceMe,
    Item::Hide,
    Item::Close,
];

impl Item {
    fn label(self) -> &'static str {
        match self {
            Item::Use => "Use this window",
            Item::Fullscreen => "Fullscreen",
            Item::Move => "Move (carry it)",
            Item::Bigger => "Show bigger",
            Item::Smaller => "Show smaller",
            Item::MoreRows => "More rows / columns",
            Item::FewerRows => "Fewer rows / columns",
            Item::Rotate => "Turn 90 degrees",
            Item::FaceMe => "Face me",
            Item::Hide => "Hide",
            Item::Close => "Close",
        }
    }

    /// A line under this entry separates groups.
    fn ends_group(self) -> bool {
        matches!(self, Item::Move | Item::FewerRows | Item::FaceMe)
    }
}

/// Layout in logical px.
const WIDTH: i32 = 330;
const PAD: i32 = 6;
const TITLE_H: i32 = 28;
const ROW_H: i32 = 27;
const SEP: i32 = 7;
const KEY_COLS: usize = 18;

pub struct PanelMenu {
    pub panel: PanelId,
    /// Top-left corner, output-relative logical px.
    pub origin: (i32, i32),
    pub hover: Option<usize>,
    title: String,
    pub image: Option<Image>,
    drawn: Option<(Option<usize>, i32)>,
}

/// Top of each entry relative to the menu box, and the box height.
fn rows() -> (Vec<i32>, i32) {
    let mut y = PAD + TITLE_H;
    let mut tops = Vec::with_capacity(ITEMS.len());
    for item in ITEMS {
        tops.push(y);
        y += ROW_H;
        if item.ends_group() {
            y += SEP;
        }
    }
    (tops, y + PAD)
}

pub fn menu_size() -> (i32, i32) {
    (WIDTH, rows().1)
}

/// The entry at a point relative to the menu's top-left corner.
pub fn item_at(p: (f64, f64)) -> Option<usize> {
    if p.0 < 0.0 || p.0 >= WIDTH as f64 {
        return None;
    }
    rows().0.iter().position(|&top| p.1 >= top as f64 && p.1 < (top + ROW_H) as f64)
}

/// Where a menu opened at `at` goes so it stays on an output of `output` size.
pub fn place(at: (i32, i32), output: (i32, i32)) -> (i32, i32) {
    let (w, h) = menu_size();
    let x = (at.0 + 6).min(output.0 - w - 4).max(0);
    let y = (at.1 + 6).min(output.1 - h - 4).max(0);
    (x, y)
}

fn draw(title: &str, keys: &[String], hover: Option<usize>, scale: i32) -> Option<Image> {
    let s = scale;
    let (w, h) = menu_size();
    let mut c = Canvas::new(w * s, h * s);
    c.fill(0, 0, w * s, h * s, [0.08, 0.09, 0.11, 0.96]);
    c.frame(0, 0, w * s, h * s, s, [0.32, 0.36, 0.42, 1.0]);
    let style = Style {
        size: 14.0,
        fg: [0.95, 0.96, 0.98, 1.0],
        bg: [0.0; 4],
        padding: 0,
        max_width: 4000,
    };
    let short: String = title.chars().take(36).collect();
    let t = text::render_lines(
        &[Line { text: short, fg: Some([0.60, 0.64, 0.70, 1.0]), bg: None }],
        Style { size: 13.0, ..style },
        s,
        0,
    )?;
    c.blit(&t.rgba, t.width, t.height, (PAD + 8) * s, (PAD + 6) * s);
    let (tops, _) = rows();
    for (i, item) in ITEMS.iter().enumerate() {
        let y = tops[i];
        if hover == Some(i) {
            c.fill(PAD * s, y * s, (w - 2 * PAD) * s, ROW_H * s, [0.20, 0.40, 0.70, 1.0]);
        }
        let color =
            if *item == Item::Close { [1.0, 0.55, 0.50, 1.0] } else { [0.95, 0.96, 0.98, 1.0] };
        let label = text::render_lines(
            &[Line { text: item.label().into(), fg: Some(color), bg: None }],
            style,
            s,
            0,
        )?;
        c.blit(&label.rgba, label.width, label.height, (PAD + 10) * s, (y + 5) * s);
        let key: String =
            keys.get(i).map(String::as_str).unwrap_or("").chars().take(KEY_COLS).collect();
        if !key.is_empty() {
            let k = text::render_lines(
                &[Line { text: key, fg: Some([0.55, 0.60, 0.66, 1.0]), bg: None }],
                Style { size: 12.0, ..style },
                s,
                0,
            )?;
            c.blit(&k.rgba, k.width, k.height, (w - PAD - 10) * s - k.width, (y + 7) * s);
        }
        if item.ends_group() {
            let ly = y + ROW_H + SEP / 2;
            c.fill((PAD + 6) * s, ly * s, (w - 2 * PAD - 12) * s, s, [0.25, 0.28, 0.33, 1.0]);
        }
    }
    let size = (w, h);
    let buffer = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
        &c.rgba,
        smithay::backend::allocator::Fourcc::Abgr8888,
        (c.width, c.height),
        s,
        smithay::utils::Transform::Normal,
        None,
    );
    Some((buffer, size))
}

impl SpaceState {
    /// The keys shown next to each entry (the user's config, Mod as the real key).
    fn menu_keys(&self) -> Vec<String> {
        let b = &self.config.keybinds;
        let m = self.mod_name();
        let first = |bind: &crate::config::keys::Binding| {
            bind.0.first().map(|c| super::help::pretty_combo(c, m)).unwrap_or_default()
        };
        ITEMS
            .iter()
            .map(|item| match item {
                Item::Use => "click".to_string(),
                Item::Fullscreen => first(&b.fullscreen),
                Item::Move => first(&b.grab),
                Item::Bigger => first(&b.scale_up),
                Item::Smaller => first(&b.scale_down),
                Item::MoreRows => first(&b.resize_grow),
                Item::FewerRows => first(&b.resize_shrink),
                Item::Rotate => first(&b.rotate),
                Item::FaceMe => first(&b.reset_rotation),
                Item::Hide => first(&b.hide),
                Item::Close => first(&b.close),
            })
            .collect()
    }

    /// Open the menu for `panel` at the crosshair (screen center).
    pub fn open_menu(&mut self, panel: PanelId) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let Some(window) = self.window_by_id(panel) else { return };
        let (app, title) = crate::state::window_names(&window);
        let title = title.filter(|t| !t.is_empty()).or(app).unwrap_or_else(|| "Window".into());
        let center = (geo.size.w / 2, geo.size.h / 2);
        let origin = place(center, (geo.size.w, geo.size.h));
        tracing::info!(panel, "window menu opened");
        self.close_inventory();
        self.view.controls.clear();
        self.pointer_location = (geo.loc + Point::from(center)).to_f64();
        self.cursor_status = CursorImageStatus::default_named();
        self.menu = Some(PanelMenu { panel, origin, hover: None, title, image: None, drawn: None });
        self.view.dirty = true;
        self.queue_redraw();
    }

    pub fn close_menu(&mut self) {
        if self.menu.take().is_some() {
            tracing::info!("window menu closed");
            self.view.dirty = true;
            self.queue_redraw();
        }
    }

    /// The menu cursor moved to `pos` (global logical).
    pub fn menu_pointer(&mut self, pos: Point<f64, Logical>) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let max: Point<f64, Logical> = Point::from((
            (geo.loc.x + geo.size.w) as f64 - 1.0,
            (geo.loc.y + geo.size.h) as f64 - 1.0,
        ));
        let pos = Point::from((
            pos.x.clamp(geo.loc.x as f64, max.x),
            pos.y.clamp(geo.loc.y as f64, max.y),
        ));
        self.pointer_location = pos;
        if let Some(m) = self.menu.as_mut() {
            let local =
                pos - geo.loc.to_f64() - Point::from((m.origin.0 as f64, m.origin.1 as f64));
            m.hover = item_at((local.x, local.y));
        }
        self.queue_redraw();
    }

    /// A click while the menu is open.
    pub fn menu_click(&mut self) {
        let hover = self.menu.as_ref().and_then(|m| m.hover);
        match hover {
            Some(i) => self.menu_activate(i),
            None => self.close_menu(),
        }
    }

    /// Up/Down move the highlight, Enter does it, Escape closes.
    pub fn menu_key(&mut self, sym: smithay::input::keyboard::Keysym) {
        use smithay::input::keyboard::Keysym;
        let n = ITEMS.len();
        match sym {
            Keysym::Escape => self.close_menu(),
            Keysym::Return | Keysym::KP_Enter => {
                if let Some(i) = self.menu.as_ref().and_then(|m| m.hover) {
                    self.menu_activate(i);
                }
            }
            Keysym::Down | Keysym::Up => {
                if let Some(m) = self.menu.as_mut() {
                    let down = sym == Keysym::Down;
                    m.hover = Some(match m.hover {
                        None if down => 0,
                        None => n - 1,
                        Some(i) if down => (i + 1) % n,
                        Some(i) => (i + n - 1) % n,
                    });
                }
                self.queue_redraw();
            }
            _ => {}
        }
    }

    fn menu_activate(&mut self, index: usize) {
        let Some(panel) = self.menu.as_ref().map(|m| m.panel) else { return };
        let Some(&item) = ITEMS.get(index) else { return };
        self.close_menu();
        tracing::info!(panel, ?item, "window menu");
        match item {
            Item::Use => set_mode(self, InputMode::Window { panel }, None),
            Item::Fullscreen => self.enter_fullscreen(panel),
            Item::Move => world_ops::grab_panel(self, panel),
            Item::Bigger => world_ops::scale_panel(self, panel, true),
            Item::Smaller => world_ops::scale_panel(self, panel, false),
            Item::MoreRows => world_ops::resize_panel(self, panel, true),
            Item::FewerRows => world_ops::resize_panel(self, panel, false),
            Item::Rotate => world_ops::rotate_panel(self, panel, false),
            Item::FaceMe => world_ops::rotate_panel(self, panel, true),
            Item::Hide => world_ops::hide_panel(self, panel),
            Item::Close => {
                if let Some(window) = self.window_by_id(panel) {
                    world_ops::close_window(&window);
                }
            }
        }
        self.queue_redraw();
    }

    /// Before each frame: redraw the menu image if the highlight changed.
    pub fn refresh_menu(&mut self) {
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        let keys = if self.menu.is_some() { self.menu_keys() } else { return };
        if let Some(m) = self.menu.as_mut() {
            if m.drawn != Some((m.hover, scale)) {
                m.image = draw(&m.title, &keys, m.hover, scale);
                m.drawn = Some((m.hover, scale));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_follows_rows_and_separators() {
        let (tops, h) = rows();
        assert_eq!(tops.len(), ITEMS.len());
        assert_eq!(item_at((20.0, tops[0] as f64 + 1.0)), Some(0));
        // The gap after "Move" is no entry.
        let mv = ITEMS.iter().position(|i| *i == Item::Move).expect("move");
        assert_eq!(item_at((20.0, (tops[mv] + ROW_H) as f64 + 1.0)), None);
        assert_eq!(item_at((20.0, tops[mv + 1] as f64 + 1.0)), Some(mv + 1));
        assert_eq!(item_at((20.0, (h - 1) as f64)), None);
        assert_eq!(item_at((-1.0, tops[0] as f64 + 1.0)), None);
        assert_eq!(item_at((20.0, 2.0)), None, "title");
    }

    #[test]
    fn stays_on_screen() {
        let (w, h) = menu_size();
        assert_eq!(place((10, 10), (1280, 720)), (16, 16));
        let (x, y) = place((1270, 710), (1280, 720));
        assert!(x + w <= 1280 && y + h <= 720 && x >= 0 && y >= 0);
    }

    #[test]
    fn renders_with_keys() {
        let keys: Vec<String> = ITEMS.iter().map(|_| "Alt+X".to_string()).collect();
        let (_, size) = draw("foot", &keys, Some(2), 1).expect("font");
        assert_eq!(size, menu_size());
    }
}
