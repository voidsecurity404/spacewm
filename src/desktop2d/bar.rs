//! The top bar on the 2D desktop: workspaces on the left (click to switch) and the
//! focused window's title, the clock and date in the middle, keyboard layout, volume,
//! Wi-Fi and battery on the right. Icons are drawn from simple shapes (the bundled font
//! has no emoji). Re-rendered only when what it shows changes.

use crate::{
    inventory::{draw::Canvas, Image},
    state::SpaceState,
};

pub const HEIGHT: i32 = 30;
const BUTTON: i32 = 28;

#[derive(Default)]
pub struct Bar {
    pub image: Option<Image>,
    /// Something it shows changed outside the per-frame check (workspaces, focus).
    pub dirty: bool,
    drawn: Option<Content>,
    /// Workspace buttons: (workspace, x from, x to) in logical px.
    buttons: Vec<(u8, i32, i32)>,
    /// The volume item's x range (click: mute; the wheel anywhere on the bar: louder /
    /// quieter).
    volume_at: Option<(i32, i32)>,
}

/// Everything the bar shows; a change means a redraw.
#[derive(Debug, Clone, PartialEq)]
struct Content {
    width: i32,
    scale: i32,
    workspaces: Vec<(u8, bool, bool)>,
    title: String,
    clock: String,
    layout: Option<String>,
    volume: Option<(u32, bool)>,
    wifi: Option<u8>,
    battery: Option<crate::desktop::status::Battery>,
}

const FG: [f32; 4] = [0.92, 0.93, 0.95, 1.0];
const DIM: [f32; 4] = [0.55, 0.58, 0.63, 1.0];
const ACCENT: [f32; 4] = [0.25, 0.45, 0.78, 1.0];

fn battery_icon(c: &mut Canvas, x: i32, y: i32, s: i32, percent: u32, charging: bool) {
    let (w, h) = (22 * s, 11 * s);
    let color = match percent {
        _ if charging => [0.40, 0.80, 0.45, 1.0],
        0..=15 => [0.92, 0.30, 0.25, 1.0],
        16..=35 => [0.95, 0.75, 0.25, 1.0],
        _ => FG,
    };
    c.frame(x, y, w, h, s, DIM);
    c.fill(x + w, y + 3 * s, 2 * s, 5 * s, DIM);
    let fill = ((w - 4 * s) * percent.min(100) as i32) / 100;
    c.fill(x + 2 * s, y + 2 * s, fill, h - 4 * s, color);
}

/// Four bars, as many lit as the signal is strong.
fn wifi_icon(c: &mut Canvas, x: i32, y: i32, s: i32, percent: u8) {
    let lit = match percent {
        0..=20 => 1,
        21..=45 => 2,
        46..=70 => 3,
        _ => 4,
    };
    for i in 0..4 {
        let h = (3 + i * 3) * s;
        let color = if i < lit { FG } else { [0.30, 0.32, 0.36, 1.0] };
        c.fill(x + i * 4 * s, y + 12 * s - h, 3 * s, h, color);
    }
}

/// A small speaker; muted: a slash through it.
fn speaker_icon(c: &mut Canvas, x: i32, y: i32, s: i32, muted: bool) {
    let color = if muted { DIM } else { FG };
    c.fill(x, y + 4 * s, 4 * s, 5 * s, color);
    for i in 0..5 {
        c.fill(x + (4 + i) * s, y + (4 - i) * s, s, (5 + 2 * i) * s, color);
    }
    if muted {
        for i in 0..10 {
            c.fill(x + i * s, y + (12 - i) * s, s, s, [0.92, 0.30, 0.25, 1.0]);
        }
    }
}

/// The image, the workspace buttons and the volume item's x range.
type Drawn = (Option<Image>, Vec<(u8, i32, i32)>, Option<(i32, i32)>);

fn draw(content: &Content) -> Drawn {
    let s = content.scale;
    let w = content.width;
    let mut c = Canvas::new(w * s, HEIGHT * s);
    c.fill(0, 0, w * s, HEIGHT * s, [0.06, 0.07, 0.09, 0.94]);
    c.fill(0, (HEIGHT - 1) * s, w * s, s, [0.20, 0.23, 0.28, 1.0]);
    let text_y = 6 * s;
    // Workspaces.
    let mut x = 6;
    let mut buttons = Vec::new();
    for (ws, current, occupied) in &content.workspaces {
        if *current {
            c.fill(x * s, 3 * s, BUTTON * s, (HEIGHT - 6) * s, ACCENT);
        }
        let label = ws.to_string();
        let tw = Canvas::text_width(&label, 14.0, s);
        let color = if *current || *occupied { FG } else { DIM };
        c.text(&label, 14.0, color, x * s + (BUTTON * s - tw) / 2, text_y, s);
        if *occupied && !*current {
            c.fill((x + BUTTON / 2 - 2) * s, (HEIGHT - 7) * s, 4 * s, 2 * s, FG);
        }
        buttons.push((*ws, x, x + BUTTON));
        x += BUTTON + 2;
    }
    // The focused window's title, after the workspaces.
    let title: String = content.title.chars().take(60).collect();
    if !title.is_empty() {
        c.text(&title, 13.0, DIM, (x + 12) * s, text_y + s, s);
    }
    // Clock in the middle.
    let cw = Canvas::text_width(&content.clock, 14.0, s);
    c.text(&content.clock, 14.0, FG, (w * s - cw) / 2, text_y, s);
    // Right side, right to left.
    let mut rx = (w - 10) * s;
    if let Some(b) = content.battery {
        let label = format!("{}%", b.percent);
        let tw = Canvas::text_width(&label, 14.0, s);
        rx -= tw;
        c.text(&label, 14.0, FG, rx, text_y, s);
        rx -= 30 * s;
        battery_icon(&mut c, rx, 10 * s, s, b.percent, b.charging);
        rx -= 16 * s;
    }
    if let Some(level) = content.wifi {
        rx -= 16 * s;
        wifi_icon(&mut c, rx, 8 * s, s, level);
        rx -= 16 * s;
    }
    let mut volume_at = None;
    if let Some((vol, muted)) = content.volume {
        let label = if muted { "mute".to_string() } else { format!("{vol}%") };
        let tw = Canvas::text_width(&label, 14.0, s);
        let end = rx;
        rx -= tw;
        c.text(&label, 14.0, if muted { DIM } else { FG }, rx, text_y, s);
        rx -= 16 * s;
        speaker_icon(&mut c, rx, 8 * s, s, muted);
        volume_at = Some((rx / s - 4, end / s + 4));
        rx -= 16 * s;
    }
    if let Some(tag) = &content.layout {
        let tw = Canvas::text_width(tag, 14.0, s);
        rx -= tw;
        c.text(tag, 14.0, FG, rx, text_y, s);
    }
    let buffer = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
        &c.rgba,
        smithay::backend::allocator::Fourcc::Abgr8888,
        (c.width, c.height),
        s,
        smithay::utils::Transform::Normal,
        None,
    );
    (Some((buffer, (w, HEIGHT))), buttons, volume_at)
}

impl SpaceState {
    /// Before each frame in 2D: redraw the bar if what it shows changed.
    pub fn refresh_bar(&mut self) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        let current = self.desktop2d.current.clamp(1, super::WORKSPACES);
        let occupied = self.desktop2d.occupied();
        let mut shown: Vec<u8> = occupied.clone();
        if !shown.contains(&current) {
            shown.push(current);
        }
        shown.sort_unstable();
        let workspaces = shown.iter().map(|&w| (w, w == current, occupied.contains(&w))).collect();
        let title = match self.seat.get_keyboard().and_then(|k| k.current_focus()) {
            Some(crate::focus::KeyboardFocusTarget::Window(w)) => {
                let (app, title) = crate::state::window_names(&w);
                title.filter(|t| !t.is_empty()).or(app).unwrap_or_default()
            }
            _ => String::new(),
        };
        let (_, month, day, weekday, h, m, _) = crate::desktop::status::local_now();
        let clock =
            format!("{}   {h:02}:{m:02}", crate::desktop::status::short_date(weekday, day, month));
        let content = Content {
            width: geo.size.w,
            scale,
            workspaces,
            title,
            clock,
            layout: self.hud.layout_tag.clone(),
            volume: self.desktop.volume,
            wifi: self.desktop.wifi,
            battery: self.desktop.battery,
        };
        if !self.desktop2d.bar.dirty && self.desktop2d.bar.drawn.as_ref() == Some(&content) {
            return;
        }
        let (image, buttons, volume_at) = draw(&content);
        let bar = &mut self.desktop2d.bar;
        bar.image = image;
        bar.buttons = buttons;
        bar.volume_at = volume_at;
        bar.drawn = Some(content);
        bar.dirty = false;
    }

    /// A click on the bar (output-relative x): switch workspace if it hit a button.
    pub fn bar_click(&mut self, x: f64) {
        let hit = self
            .desktop2d
            .bar
            .buttons
            .iter()
            .find(|(_, a, b)| x >= *a as f64 && x < *b as f64)
            .map(|(ws, _, _)| *ws);
        if let Some(ws) = hit {
            self.switch_workspace(ws);
        } else if self.desktop2d.bar.volume_at.is_some_and(|(a, b)| x >= a as f64 && x < b as f64) {
            self.hardware_key(crate::desktop::Hw::VolumeMute);
        }
    }

    /// The wheel on the bar: volume up (wheel up) / down.
    pub fn bar_scroll(&mut self, notches: f64) {
        if notches < 0.0 {
            self.hardware_key(crate::desktop::Hw::VolumeUp);
        } else if notches > 0.0 {
            self.hardware_key(crate::desktop::Hw::VolumeDown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_and_finds_buttons() {
        let content = Content {
            width: 1280,
            scale: 1,
            workspaces: vec![(1, false, true), (2, true, true), (5, false, true)],
            title: "foot".into(),
            clock: "Sat 24 Oct   21:32".into(),
            layout: Some("EN".into()),
            volume: Some((60, false)),
            wifi: Some(80),
            battery: Some(crate::desktop::status::Battery { percent: 87, charging: false }),
        };
        let (image, buttons, volume_at) = draw(&content);
        let (_, size) = image.expect("drawn");
        let (a, b) = volume_at.expect("volume shown");
        assert!(a > 900 && b > a && b < 1280, "{a}..{b}");
        assert_eq!(size, (1280, HEIGHT));
        assert_eq!(buttons.iter().map(|b| b.0).collect::<Vec<_>>(), vec![1, 2, 5]);
        assert!(buttons.windows(2).all(|p| p[0].2 <= p[1].1), "buttons don't overlap");
    }
}
