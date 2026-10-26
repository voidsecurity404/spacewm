//! The emote menu (F6 while walking): nine animations in a 3x3 grid. A number key
//! (1-9), a click, or the arrows and Enter play one (world/body.rs); Escape or F6
//! closes it.

use smithay::{
    input::{keyboard::Keysym, pointer::CursorImageStatus},
    utils::{Logical, Point},
};

use crate::{
    inventory::{draw::Canvas, Image},
    state::SpaceState,
    world::body::EMOTES,
};

/// Layout in logical px.
const CELL: i32 = 150;
const GAP: i32 = 8;
const PAD: i32 = 16;
const TITLE_H: i32 = 34;

pub struct EmoteMenu {
    pub selected: usize,
    pub image: Option<Image>,
    drawn: Option<(usize, i32)>,
}

pub fn box_size() -> (i32, i32) {
    let grid = 3 * CELL + 2 * GAP;
    (grid + 2 * PAD, TITLE_H + grid + 2 * PAD)
}

pub fn box_origin(output: (i32, i32)) -> (i32, i32) {
    let (w, h) = box_size();
    ((output.0 - w) / 2, ((output.1 - h) / 2).max(0))
}

/// The cell at `p` (box-relative), 0..9.
pub fn cell_at(p: (f64, f64)) -> Option<usize> {
    let x = p.0 - PAD as f64;
    let y = p.1 - (PAD + TITLE_H) as f64;
    if x < 0.0 || y < 0.0 {
        return None;
    }
    let step = (CELL + GAP) as f64;
    let (c, r) = ((x / step) as usize, (y / step) as usize);
    // Not in the gap between two cells.
    let inside = x % step < CELL as f64 && y % step < CELL as f64;
    (c < 3 && r < 3 && inside).then_some(r * 3 + c)
}

impl SpaceState {
    pub fn toggle_emotes(&mut self) {
        if self.emotes.take().is_some() {
            self.view.dirty = true;
            self.queue_redraw();
            return;
        }
        tracing::info!("emote menu opened");
        self.close_menu();
        self.set_dock_cursor(false);
        self.view.controls.clear();
        if let Some(geo) = self.primary_output_geometry() {
            self.pointer_location = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
        }
        self.cursor_status = CursorImageStatus::default_named();
        self.emotes = Some(EmoteMenu { selected: 0, image: None, drawn: None });
        self.queue_redraw();
    }

    fn pick_emote(&mut self, i: usize) {
        self.emotes = None;
        self.view.dirty = true;
        self.play_emote(i);
    }

    pub fn emotes_key(&mut self, sym: Keysym) {
        let Some(sel) = self.emotes.as_ref().map(|m| m.selected) else { return };
        let raw = sym.raw();
        let digit = (Keysym::_1.raw()..=Keysym::_9.raw())
            .contains(&raw)
            .then(|| (raw - Keysym::_1.raw()) as usize)
            .or_else(|| {
                (Keysym::KP_1.raw()..=Keysym::KP_9.raw())
                    .contains(&raw)
                    .then(|| (raw - Keysym::KP_1.raw()) as usize)
            });
        if let Some(i) = digit {
            self.pick_emote(i);
            return;
        }
        let moved = match sym {
            Keysym::Escape | Keysym::F6 => {
                self.toggle_emotes();
                return;
            }
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                self.pick_emote(sel);
                return;
            }
            Keysym::Left => (sel + 8) % 9,
            Keysym::Right => (sel + 1) % 9,
            Keysym::Up => (sel + 6) % 9,
            Keysym::Down => (sel + 3) % 9,
            _ => return,
        };
        if let Some(m) = self.emotes.as_mut() {
            m.selected = moved;
        }
        self.queue_redraw();
    }

    fn emotes_local(&self) -> Option<(f64, f64)> {
        let geo = self.primary_output_geometry()?;
        let o = box_origin((geo.size.w, geo.size.h));
        let local =
            self.pointer_location - geo.loc.to_f64() - Point::from((o.0 as f64, o.1 as f64));
        Some((local.x, local.y))
    }

    pub fn emotes_pointer(&mut self, pos: Point<f64, Logical>) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let max: Point<f64, Logical> = Point::from((
            (geo.loc.x + geo.size.w) as f64 - 1.0,
            (geo.loc.y + geo.size.h) as f64 - 1.0,
        ));
        self.pointer_location = Point::from((
            pos.x.clamp(geo.loc.x as f64, max.x),
            pos.y.clamp(geo.loc.y as f64, max.y),
        ));
        if let Some(i) = self.emotes_local().and_then(cell_at) {
            if let Some(m) = self.emotes.as_mut() {
                m.selected = i;
            }
        }
        self.queue_redraw();
    }

    /// A click on a cell plays it; elsewhere closes the menu.
    pub fn emotes_click(&mut self) {
        match self.emotes_local().and_then(cell_at) {
            Some(i) => self.pick_emote(i),
            None => self.toggle_emotes(),
        }
    }

    pub fn refresh_emotes(&mut self) {
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        let Some(m) = self.emotes.as_ref() else { return };
        if m.drawn == Some((m.selected, scale)) {
            return;
        }
        let selected = m.selected;
        let image = draw(selected, scale);
        if let Some(m) = self.emotes.as_mut() {
            m.image = image;
            m.drawn = Some((selected, scale));
        }
    }
}

fn draw(selected: usize, scale: i32) -> Option<Image> {
    let s = scale;
    let (w, h) = box_size();
    let mut c = Canvas::new(w * s, h * s);
    c.fill(0, 0, w * s, h * s, [0.07, 0.08, 0.10, 0.94]);
    c.frame(0, 0, w * s, h * s, 2 * s, [0.95, 0.70, 0.25, 1.0]);
    c.text("Emotes  (1-9, click; Esc closes)", 16.0, [1.0, 0.86, 0.45, 1.0], PAD * s, PAD * s, s);
    for (i, e) in EMOTES.iter().enumerate() {
        let (col, row) = ((i % 3) as i32, (i / 3) as i32);
        let x = PAD + col * (CELL + GAP);
        let y = PAD + TITLE_H + row * (CELL + GAP);
        let fill = if i == selected { [0.20, 0.42, 0.78, 1.0] } else { [0.15, 0.17, 0.20, 1.0] };
        c.fill(x * s, y * s, CELL * s, CELL * s, fill);
        c.frame(x * s, y * s, CELL * s, CELL * s, s, [0.40, 0.45, 0.52, 1.0]);
        let num = format!("{}", i + 1);
        let nw = Canvas::text_width(&num, 40.0, s);
        c.text(&num, 40.0, [1.0, 1.0, 1.0, 0.9], x * s + (CELL * s - nw) / 2, (y + 30) * s, s);
        let lw = Canvas::text_width(e.label, 16.0, s);
        c.text(
            e.label,
            16.0,
            [0.92, 0.94, 0.97, 1.0],
            x * s + (CELL * s - lw) / 2,
            (y + 100) * s,
            s,
        );
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
    fn grid_hit_test() {
        let y0 = (PAD + TITLE_H) as f64;
        assert_eq!(cell_at((PAD as f64 + 5.0, y0 + 5.0)), Some(0));
        let step = (CELL + GAP) as f64;
        assert_eq!(cell_at((PAD as f64 + 2.0 * step + 5.0, y0 + 2.0 * step + 5.0)), Some(8));
        assert_eq!(cell_at((PAD as f64 + CELL as f64 + 2.0, y0 + 5.0)), None, "gap");
        assert_eq!(cell_at((2.0, 2.0)), None, "title");
        let (w, h) = box_size();
        assert!(w <= 1280 && h <= 720);
    }

    #[test]
    fn renders() {
        let (_, size) = draw(4, 1).expect("font");
        assert_eq!(size, box_size());
    }
}
