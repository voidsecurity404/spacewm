//! Building blocks: the hotbar along the bottom of the screen and the inventory screen
//! (E while walking), Minecraft style.
//!
//! - Hotbar: one slot per placeable block, the selected one framed in white. Keys 1..5 or
//!   the scroll wheel pick a slot; right click places that block, left click breaks the
//!   block under the crosshair.
//! - Inventory: a box in the middle of the screen with every block, its name and number.
//!   While it is open the mouse moves an ordinary cursor (the view stays still); a click
//!   on a block selects it and closes the box. E or Escape closes it too.
//!
//! Both are 2D overlays drawn on the CPU (draw.rs); they are re-rendered only when what
//! they show changes.

pub mod draw;

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorImageStatus,
    utils::{Logical, Point, Size, Transform},
};

use crate::{state::SpaceState, world::block::Block};
use draw::Canvas;

/// Inventory layout in logical px.
const SLOT: i32 = 64;
const GAP: i32 = 10;
const PAD: i32 = 18;
const TITLE_H: i32 = 30;
const NAME_H: i32 = 26;
const HINT_H: i32 = 22;
/// Hotbar layout in logical px.
const BAR_SLOT: i32 = 46;
const BAR_ICON: i32 = 32;
/// Gap between the hotbar and the bottom edge of the screen.
pub const BAR_MARGIN: i32 = 14;

pub type Image = (MemoryRenderBuffer, (i32, i32));

#[derive(Default)]
pub struct Inventory {
    /// The slot under the cursor.
    pub hover: Option<usize>,
    pub image: Option<Image>,
    /// (hover, selected, scale) the image was drawn for.
    drawn: Option<(Option<usize>, usize, i32)>,
}

#[derive(Default)]
pub struct Hotbar {
    pub image: Option<Image>,
    drawn: Option<(usize, i32)>,
}

/// Size of the inventory box in logical px.
pub fn inventory_size() -> (i32, i32) {
    let n = Block::PLACEABLE.len() as i32;
    (2 * PAD + n * SLOT + (n - 1) * GAP, 2 * PAD + TITLE_H + SLOT + NAME_H + HINT_H)
}

/// Top-left corner of the inventory box on an output of `output` logical size: centered.
pub fn inventory_origin(output: (i32, i32)) -> (i32, i32) {
    let (w, h) = inventory_size();
    ((output.0 - w) / 2, ((output.1 - h) / 2).max(0))
}

/// Top-left corner of the hotbar.
pub fn hotbar_origin(output: (i32, i32), size: (i32, i32)) -> (i32, i32) {
    ((output.0 - size.0) / 2, output.1 - size.1 - BAR_MARGIN)
}

/// Which slot a point (logical px, relative to the inventory box) is on.
pub fn slot_at(p: (f64, f64)) -> Option<usize> {
    let y0 = (PAD + TITLE_H) as f64;
    if p.1 < y0 || p.1 >= y0 + SLOT as f64 || p.0 < PAD as f64 {
        return None;
    }
    let x = p.0 - PAD as f64;
    let i = (x / (SLOT + GAP) as f64) as usize;
    let within = x - ((i as i32 * (SLOT + GAP)) as f64) < SLOT as f64;
    (within && i < Block::PLACEABLE.len()).then_some(i)
}

fn to_buffer(c: Canvas, scale: i32) -> Image {
    let size = (c.width / scale, c.height / scale);
    let buffer = MemoryRenderBuffer::from_slice(
        &c.rgba,
        // Bytes are R,G,B,A in memory: ABGR8888 in DRM's little-endian naming.
        Fourcc::Abgr8888,
        (c.width, c.height),
        scale,
        Transform::Normal,
        None,
    );
    (buffer, size)
}

fn draw_inventory(hover: Option<usize>, selected: usize, scale: i32) -> Image {
    let s = scale;
    let (w, h) = inventory_size();
    let mut c = Canvas::new(w * s, h * s);
    // Light gray box with a dark outline, like Minecraft's container screens.
    c.fill(0, 0, w * s, h * s, [0.78, 0.78, 0.78, 0.97]);
    c.frame(0, 0, w * s, h * s, 2 * s, [0.1, 0.1, 0.1, 1.0]);
    c.text("Inventory", 17.0, [0.25, 0.25, 0.25, 1.0], PAD * s, (PAD - 4) * s, s);
    let y = (PAD + TITLE_H) * s;
    for (i, block) in Block::PLACEABLE.iter().enumerate() {
        let x = (PAD + i as i32 * (SLOT + GAP)) * s;
        let fill = if hover == Some(i) { [0.70, 0.70, 0.72, 1.0] } else { [0.55, 0.55, 0.55, 1.0] };
        c.inset(x, y, SLOT * s, SLOT * s, 2 * s, fill);
        let icon = (SLOT - 18) * s;
        c.blit(&draw::block_icon(*block, icon), icon, icon, x + 9 * s, y + 9 * s);
        c.text(&(i + 1).to_string(), 12.0, [1.0, 1.0, 1.0, 1.0], x + 5 * s, y + 3 * s, s);
        if i == selected {
            c.frame(x - 2 * s, y - 2 * s, (SLOT + 4) * s, (SLOT + 4) * s, 2 * s, [1.0; 4]);
        }
    }
    let name = Block::PLACEABLE[hover.unwrap_or(selected)].name();
    let size = 15.0;
    let nx = (w * s - Canvas::text_width(name, size, s)) / 2;
    c.text(name, size, [0.15, 0.15, 0.15, 1.0], nx, y + (SLOT + 6) * s, s);
    let hint = "click or 1-5: choose    E: close";
    let hx = (w * s - Canvas::text_width(hint, 12.0, s)) / 2;
    c.text(hint, 12.0, [0.35, 0.35, 0.35, 1.0], hx, y + (SLOT + NAME_H + 6) * s, s);
    to_buffer(c, s)
}

fn draw_hotbar(selected: usize, scale: i32) -> Image {
    let s = scale;
    let n = Block::PLACEABLE.len() as i32;
    let (w, h) = (n * BAR_SLOT + 4, BAR_SLOT + 4);
    let mut c = Canvas::new(w * s, h * s);
    c.fill(0, 0, w * s, h * s, [0.0, 0.0, 0.0, 0.45]);
    c.frame(0, 0, w * s, h * s, 2 * s, [0.35, 0.35, 0.35, 0.9]);
    for (i, block) in Block::PLACEABLE.iter().enumerate() {
        let x = (2 + i as i32 * BAR_SLOT) * s;
        let pad = (BAR_SLOT - BAR_ICON) / 2;
        let icon = BAR_ICON * s;
        c.blit(&draw::block_icon(*block, icon), icon, icon, x + pad * s, (2 + pad) * s);
        c.text(&(i + 1).to_string(), 11.0, [0.9, 0.9, 0.9, 0.9], x + 3 * s, 3 * s, s);
        if i == selected {
            c.frame(x - s, s, (BAR_SLOT + 2) * s, (BAR_SLOT + 2) * s, 3 * s, [1.0; 4]);
        }
    }
    to_buffer(c, s)
}

impl SpaceState {
    /// The hotbar slot of the block selected for placing.
    pub fn selected_slot(&self) -> usize {
        Block::PLACEABLE.iter().position(|b| *b == self.voxels.selected).unwrap_or(0)
    }

    pub fn select_slot(&mut self, slot: usize) {
        if let Some(&b) = Block::PLACEABLE.get(slot) {
            self.voxels.selected = b;
            tracing::info!(block = ?b, "block selected");
            self.queue_redraw();
        }
    }

    /// Scroll wheel while walking: next/previous hotbar slot, wrapping around.
    pub fn scroll_slot(&mut self, notches: i32) {
        let n = Block::PLACEABLE.len() as i32;
        let slot = (self.selected_slot() as i32 + notches).rem_euclid(n);
        self.select_slot(slot as usize);
    }

    fn output_scale(&self) -> i32 {
        self.space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1)
    }

    pub fn toggle_inventory(&mut self) {
        if self.inventory.is_some() {
            self.close_inventory();
            return;
        }
        if self.render_mode != crate::cli::RenderMode::Scene3D
            || self.lock.is_locked()
            || self.mode != crate::input::mode::InputMode::Walk
        {
            return;
        }
        tracing::info!("inventory opened");
        self.inventory = Some(Inventory::default());
        // Walking stops; an ordinary arrow cursor starts in the middle of the screen.
        self.view.controls.clear();
        if let Some(geo) = self.primary_output_geometry() {
            self.pointer_location = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
        }
        self.cursor_status = CursorImageStatus::default_named();
        self.view.dirty = true;
        self.queue_redraw();
    }

    pub fn close_inventory(&mut self) {
        if self.inventory.take().is_some() {
            tracing::info!("inventory closed");
            self.view.dirty = true;
            self.queue_redraw();
        }
    }

    /// The inventory cursor moved to `pos` (global logical coordinates).
    pub fn inventory_pointer(&mut self, pos: Point<f64, Logical>) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let max =
            geo.loc.to_f64() + Point::from((geo.size.w as f64 - 1.0, geo.size.h as f64 - 1.0));
        let pos = Point::from((
            pos.x.clamp(geo.loc.x as f64, max.x),
            pos.y.clamp(geo.loc.y as f64, max.y),
        ));
        self.pointer_location = pos;
        let origin = inventory_origin((geo.size.w, geo.size.h));
        let local = pos - geo.loc.to_f64() - Point::from((origin.0 as f64, origin.1 as f64));
        let hover = slot_at((local.x, local.y));
        if let Some(inv) = self.inventory.as_mut() {
            inv.hover = hover;
        }
        self.queue_redraw();
    }

    /// A click while the inventory is open: pick the block under the cursor, or close the
    /// box when clicking outside it.
    pub fn inventory_click(&mut self) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let origin = inventory_origin((geo.size.w, geo.size.h));
        let (w, h) = inventory_size();
        let local = self.pointer_location
            - geo.loc.to_f64()
            - Point::from((origin.0 as f64, origin.1 as f64));
        let inside = local.x >= 0.0 && local.y >= 0.0 && local.x < w as f64 && local.y < h as f64;
        match slot_at((local.x, local.y)) {
            Some(slot) => {
                self.select_slot(slot);
                self.close_inventory();
            }
            None if !inside => self.close_inventory(),
            None => {}
        }
    }

    /// Before every frame: redraw the hotbar/inventory images if what they show changed.
    pub fn refresh_inventory(&mut self) {
        let scale = self.output_scale();
        let selected = self.selected_slot();
        let show_bar = self.render_mode == crate::cli::RenderMode::Scene3D
            && self.hud.visible
            && self.mode == crate::input::mode::InputMode::Walk
            && self.fullscreen.is_none();
        if !show_bar {
            self.hotbar = Hotbar::default();
        } else if self.hotbar.drawn != Some((selected, scale)) {
            self.hotbar.image = Some(draw_hotbar(selected, scale));
            self.hotbar.drawn = Some((selected, scale));
        }
        if let Some(inv) = self.inventory.as_mut() {
            let key = (inv.hover, selected, scale);
            if inv.drawn != Some(key) {
                inv.image = Some(draw_inventory(inv.hover, selected, scale));
                inv.drawn = Some(key);
            }
        }
    }
}

/// Where overlays put the hotbar on an output of this logical size.
pub fn hotbar_rect(output: Size<i32, Logical>, image: &Image) -> (i32, i32) {
    hotbar_origin((output.w, output.h), image.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_hit_test() {
        let y = (PAD + TITLE_H + 5) as f64;
        assert_eq!(slot_at((PAD as f64 + 1.0, y)), Some(0));
        assert_eq!(slot_at(((PAD + SLOT + GAP) as f64 + 1.0, y)), Some(1));
        // The gap between slots and the space above them hit nothing.
        assert_eq!(slot_at(((PAD + SLOT) as f64 + 2.0, y)), None);
        assert_eq!(slot_at((PAD as f64 + 1.0, 2.0)), None);
        let last = Block::PLACEABLE.len() - 1;
        let x = (PAD + last as i32 * (SLOT + GAP) + SLOT - 1) as f64;
        assert_eq!(slot_at((x, y)), Some(last));
        assert_eq!(slot_at((x + (GAP + 5) as f64, y)), None);
    }

    #[test]
    fn inventory_is_centered() {
        let (w, h) = inventory_size();
        assert_eq!(inventory_origin((1280, 720)), ((1280 - w) / 2, (720 - h) / 2));
        // Tiny screens: never above the top edge.
        assert_eq!(inventory_origin((100, 50)).1, 0);
    }

    #[test]
    fn images_have_the_layout_size() {
        let (_, size) = draw_inventory(Some(1), 2, 2);
        assert_eq!(size, inventory_size());
        let (_, bar) = draw_hotbar(0, 1);
        assert_eq!(bar.0, Block::PLACEABLE.len() as i32 * BAR_SLOT + 4);
    }
}
