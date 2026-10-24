//! The on-screen display for media keys: "Volume 45%" with a bar, low on the screen, for
//! a moment after each key press.

use std::time::Duration;

use smithay::reexports::calloop::{
    timer::{TimeoutAction, Timer},
    RegistrationToken,
};

use crate::{
    inventory::{draw::Canvas, Image},
    state::SpaceState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Volume,
    Microphone,
    Brightness,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Volume => "Volume",
            Kind::Microphone => "Microphone",
            Kind::Brightness => "Brightness",
        }
    }
}

/// How long it stays after the last key press.
const SHOW_FOR: Duration = Duration::from_millis(1600);
/// Size in logical px.
const W: i32 = 280;
const H: i32 = 64;
/// Its bottom edge is this far above the screen's bottom (clear of the hotbar).
pub const BOTTOM_GAP: i32 = 110;

pub struct Osd {
    pub image: Image,
    timer: Option<RegistrationToken>,
}

/// The OSD text: "45%", "muted", or "" when the level couldn't be read.
pub fn value_text(percent: Option<u32>, muted: bool) -> String {
    match (muted, percent) {
        (true, _) => "muted".into(),
        (false, Some(p)) => format!("{p}%"),
        (false, None) => String::new(),
    }
}

fn draw(kind: Kind, percent: Option<u32>, muted: bool, scale: i32) -> Image {
    let c = canvas(kind, percent, muted, scale);
    let buffer = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
        &c.rgba,
        smithay::backend::allocator::Fourcc::Abgr8888,
        (c.width, c.height),
        scale,
        smithay::utils::Transform::Normal,
        None,
    );
    (buffer, (W, H))
}

fn canvas(kind: Kind, percent: Option<u32>, muted: bool, scale: i32) -> Canvas {
    let s = scale;
    let mut c = Canvas::new(W * s, H * s);
    c.fill(0, 0, W * s, H * s, [0.06, 0.07, 0.09, 0.90]);
    c.frame(0, 0, W * s, H * s, s, [0.30, 0.34, 0.40, 1.0]);
    let white = [0.95, 0.96, 0.98, 1.0];
    c.text(kind.label(), 16.0, white, 16 * s, 10 * s, s);
    let value = value_text(percent, muted);
    let vw = Canvas::text_width(&value, 16.0, s);
    c.text(&value, 16.0, white, (W - 16) * s - vw, 10 * s, s);
    // The bar: 0..100% (volume may go a bit over 100 with some tools: clamp).
    let (bx, by, bw, bh) = (16 * s, 40 * s, (W - 32) * s, 10 * s);
    c.fill(bx, by, bw, bh, [0.22, 0.24, 0.28, 1.0]);
    let fill = percent.unwrap_or(0).min(100) as i32 * bw / 100;
    let color = if muted { [0.45, 0.47, 0.50, 1.0] } else { [0.35, 0.68, 1.0, 1.0] };
    c.fill(bx, by, fill, bh, color);
    c
}

impl SpaceState {
    pub fn show_osd(&mut self, kind: Kind, percent: Option<u32>, muted: bool) {
        tracing::info!(what = kind.label(), ?percent, muted, "osd");
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        // Pressing again while it is shown: replace it and start the time over.
        if let Some(token) = self.desktop.osd.take().and_then(|o| o.timer) {
            self.loop_handle.remove(token);
        }
        let timer = self
            .loop_handle
            .insert_source(Timer::from_duration(SHOW_FOR), |_, _, state| {
                state.desktop.osd = None;
                state.queue_redraw();
                TimeoutAction::Drop
            })
            .ok();
        self.desktop.osd = Some(Osd { image: draw(kind, percent, muted, scale), timer });
        self.queue_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(value_text(Some(45), false), "45%");
        assert_eq!(value_text(Some(45), true), "muted");
        assert_eq!(value_text(None, false), "");
    }

    #[test]
    fn bar_fills_with_the_level() {
        // Bright blue pixels on the bar's middle row.
        let lit = |p, muted| {
            let c = canvas(Kind::Volume, Some(p), muted, 1);
            let row = 45 * W as usize * 4;
            c.rgba[row..row + W as usize * 4]
                .chunks(4)
                .filter(|px| px[2] > 200 && px[0] < 120)
                .count()
        };
        assert!(lit(80, false) > 3 * lit(20, false) / 2);
        assert_eq!(lit(0, false), 0);
        assert_eq!(lit(80, true), 0, "muted: a gray bar");
    }
}
