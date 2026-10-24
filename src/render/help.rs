//! The key help (Mod+K): every key spacewm uses, in two columns over the view. Built from
//! the keybinds actually in effect, so a changed config shows its own keys, and "Mod" is
//! written as the key it really is here (Alt when nested, Super on a real session).
//!
//! While it is open, keys and clicks go to it: Escape, Mod+K or a click closes it.

use super::text::{self, Line, Style};
use crate::{
    config::{keys::KeybindsConfig, ModKey},
    inventory::{draw::Canvas, Image},
    state::SpaceState,
};

/// Text size and colors.
const SIZE: f32 = 15.0;
const TITLE: [f32; 4] = [1.0, 0.86, 0.45, 1.0];
const HEADER: [f32; 4] = [0.55, 0.80, 1.0, 1.0];
const TEXT: [f32; 4] = [0.93, 0.94, 0.96, 1.0];
/// Width of the key column, in characters.
const KEY_COLS: usize = 24;

/// A key name as people know it ("equal" -> "=").
fn pretty_key(name: &str) -> String {
    match name {
        "equal" => "=".into(),
        "minus" => "-".into(),
        "Return" => "Enter".into(),
        "space" => "Space".into(),
        "grave" => "`".into(),
        "Escape" => "Esc".into(),
        "comma" => ",".into(),
        "KP_Add" => "Num+".into(),
        "KP_Subtract" => "Num-".into(),
        n if n.len() == 1 => n.to_uppercase(),
        n => n.to_string(),
    }
}

/// "Mod+Shift+equal" -> "Alt+Shift+=".
pub fn pretty_combo(combo: &str, mod_name: &str) -> String {
    combo
        .split('+')
        .map(|part| match part {
            "Mod" => mod_name.to_string(),
            "Ctrl" | "Shift" | "Alt" | "Super" => part.to_string(),
            key => pretty_key(key),
        })
        .collect::<Vec<_>>()
        .join("+")
}

fn row(keys: &str, what: &str) -> Line {
    Line { text: format!("{keys:<KEY_COLS$}{what}"), fg: Some(TEXT), bg: None }
}

fn header(text: &str) -> Line {
    Line { text: text.to_string(), fg: Some(HEADER), bg: None }
}

/// The two columns of the help box.
pub fn help_columns(
    binds: &KeybindsConfig,
    mod_name: &str,
    nested: bool,
    building: bool,
) -> (Vec<Line>, Vec<Line>) {
    // At most two combos per action, and only as many as fit the key column (scale_up
    // has three; "Mod+Ctrl+Shift+=" is long).
    let keys = |b: &crate::config::keys::Binding| {
        let mut out = String::new();
        for combo in b.0.iter().take(2).map(|c| pretty_combo(c, mod_name)) {
            let next = if out.is_empty() { combo } else { format!("{out} / {combo}") };
            if !out.is_empty() && next.chars().count() >= KEY_COLS {
                break;
            }
            out = next;
        }
        out
    };
    let mut left = vec![
        header("Walking"),
        row("W A S D", "walk"),
        row("Mouse", "look around"),
        row("Space", "jump (twice: fly)"),
        row("Shift / Ctrl", "down (flying) / run"),
        row("Left click", "use the window you look at"),
        row("Right click", "window menu"),
        row("Drag top edge", "move a window (corner: resize)"),
        row(&format!("Hold {mod_name}"), "cursor for the dock"),
        row(&keys(&binds.walk_mode), "leave a window, back to walking"),
        Line::default(),
        header("Windows"),
        row(&keys(&binds.spawn_terminal), "terminal"),
        row(&keys(&binds.launcher), "app launcher"),
        row(&keys(&binds.close), "close window"),
        row(&keys(&binds.fullscreen), "fullscreen (Esc leaves it)"),
        row(&keys(&binds.next_window), "go to next window"),
        row(&keys(&binds.seated), "seated: windows around me"),
        row(&keys(&binds.toggle_2d), "3D world / plain 2D desktop"),
        row(&format!("2D: {mod_name}+drag"), "move (right button: resize)"),
        row(&keys(&binds.grab), "carry / put down a window"),
        row(&keys(&binds.rotate), "turn a window"),
        row(&keys(&binds.reset_rotation), "face me again"),
        row(&keys(&binds.scale_up), "window looks bigger"),
        row(&keys(&binds.scale_down), "window looks smaller"),
        row(&keys(&binds.resize_grow), "more rows/columns"),
        row(&keys(&binds.resize_shrink), "fewer rows/columns"),
        row(&keys(&binds.hide), "hide window (none: show all)"),
    ];
    let mut right = Vec::new();
    if building {
        right.extend([
            header("Building"),
            row("E", "inventory"),
            row("1-5 / wheel", "pick a block"),
            row("Right click", "place block"),
            row("Left click", "break block"),
            Line::default(),
        ]);
    }
    right.extend([
        header("Desktop"),
        row(&keys(&binds.lock), "lock the screen"),
        row(&keys(&binds.screenshot), "screenshot (+ clipboard)"),
        row(&keys(&binds.screenshot_region), "screenshot of a region"),
        row("Media keys", "volume, brightness, music"),
        Line::default(),
        header("spacewm"),
        row(&keys(&binds.help), "this help"),
        row(&keys(&binds.settings), "settings"),
        row(&keys(&binds.hud), "clock line on/off"),
        row(&keys(&binds.debug), "fps in the log"),
        row(&keys(&binds.quit), "quit spacewm"),
        row("Ctrl+Alt+Backspace", "emergency exit"),
    ]);
    if nested {
        right.push(row("Ctrl+Alt+G", "give the mouse back"));
    }
    if !binds.spawn.is_empty() {
        right.push(Line::default());
        right.push(header("Your keys"));
        for s in binds.spawn.iter().take(8) {
            let command: String = s.command.chars().take(26).collect();
            right.push(row(&pretty_combo(&s.key, mod_name), &command));
        }
    }
    // Same height, so the box is a clean rectangle.
    let n = left.len().max(right.len());
    left.resize(n, Line::default());
    right.resize(n, Line::default());
    (left, right)
}

impl SpaceState {
    /// How "Mod" is written in the help: the key that really is Mod here.
    pub fn mod_name(&self) -> &'static str {
        match (self.config.general.mod_key, self.backend.is_nested()) {
            (ModKey::Alt, _) | (ModKey::Super, true) => "Alt",
            (ModKey::Super, false) => "Super",
        }
    }

    pub fn toggle_help(&mut self) {
        if self.help.is_some() {
            self.close_help();
            return;
        }
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        let (left, right) = help_columns(
            &self.config.keybinds,
            self.mod_name(),
            self.backend.is_nested(),
            self.config.general.building,
        );
        self.help = render_help(&left, &right, scale);
        if self.help.is_some() {
            tracing::info!("key help shown");
            // Walking stops while reading.
            self.view.controls.clear();
            self.view.dirty = true;
            self.queue_redraw();
        }
    }

    pub fn close_help(&mut self) {
        if self.help.take().is_some() {
            tracing::info!("key help closed");
            self.view.dirty = true;
            self.queue_redraw();
        }
    }
}

fn render_help(left: &[Line], right: &[Line], scale: i32) -> Option<Image> {
    let s = scale;
    let style = Style { size: SIZE, fg: TEXT, bg: [0.0; 4], padding: 0, max_width: 4000 };
    let title = text::render_lines(
        &[Line { text: "spacewm keys".into(), fg: Some(TITLE), bg: None }],
        Style { size: 19.0, ..style },
        s,
        0,
    )?;
    let hint = text::render_lines(
        &[Line { text: "Esc or click: close".into(), fg: Some([0.6, 0.62, 0.66, 1.0]), bg: None }],
        Style { size: 13.0, ..style },
        s,
        0,
    )?;
    let l = text::render_lines(left, style, s, 0)?;
    let r = text::render_lines(right, style, s, 0)?;
    let (pad, gap) = (22 * s, 36 * s);
    let width = (2 * pad + l.width + gap + r.width).max(2 * pad + title.width);
    let body_y = pad + title.height + 12 * s;
    let height = body_y + l.height.max(r.height) + 10 * s + hint.height + pad;
    let mut c = Canvas::new(width, height);
    c.fill(0, 0, width, height, [0.06, 0.07, 0.09, 0.94]);
    c.frame(0, 0, width, height, 2 * s, [0.30, 0.34, 0.40, 1.0]);
    c.blit(&title.rgba, title.width, title.height, pad, pad);
    c.blit(&l.rgba, l.width, l.height, pad, body_y);
    c.blit(&r.rgba, r.width, r.height, pad + l.width + gap, body_y);
    let hint_y = body_y + l.height.max(r.height) + 10 * s;
    c.blit(&hint.rgba, hint.width, hint.height, width - pad - hint.width, hint_y);
    let size = (width / s, height / s);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_read_like_keys() {
        assert_eq!(pretty_combo("Mod+Shift+equal", "Alt"), "Alt+Shift+=");
        assert_eq!(pretty_combo("Mod+Return", "Super"), "Super+Enter");
        assert_eq!(pretty_combo("Mod+grave", "Alt"), "Alt+`");
        assert_eq!(pretty_combo("Mod+f", "Alt"), "Alt+F");
        assert_eq!(pretty_combo("Mod+F3", "Alt"), "Alt+F3");
    }

    #[test]
    fn help_lists_the_configured_keys() {
        let mut binds = KeybindsConfig::default();
        binds.spawn.push(crate::config::keys::SpawnBind {
            key: "Mod+B".into(),
            command: "firefox".into(),
        });
        let (left, right) = help_columns(&binds, "Alt", true, true);
        assert_eq!(left.len(), right.len());
        let all: Vec<&str> = left.iter().chain(&right).map(|l| l.text.as_str()).collect();
        let has = |s: &str| all.iter().any(|l| l.contains(s));
        assert!(has("Alt+Enter") && has("terminal"));
        assert!(has("Alt+F ") && has("fullscreen"));
        assert!(has("Alt+K") && has("this help"));
        assert!(has("Alt+L") && has("Print") && has("Shift+Print"));
        assert!(has("Ctrl+Alt+G"), "nested: how to get the mouse back");
        // Every key fits its column: the description never touches the key.
        for l in left.iter().chain(&right).filter(|l| l.text.chars().count() > KEY_COLS) {
            assert_eq!(l.text.chars().nth(KEY_COLS - 1), Some(' '), "{:?}", l.text);
        }
        assert!(has("Alt+B") && has("firefox"), "the user's own spawn keys");
        let (_, right) = help_columns(&binds, "Super", false, false);
        assert!(!right.iter().any(|l| l.text.contains("inventory")), "building off: no block keys");
        assert!(!right.iter().any(|l| l.text.contains("Ctrl+Alt+G")));
    }

    #[test]
    fn renders() {
        let (l, r) = help_columns(&KeybindsConfig::default(), "Alt", true, true);
        let (_, size) = render_help(&l, &r, 1).expect("font");
        assert!(size.0 > 600 && size.1 > 300 && size.1 < 700, "{size:?}");
    }
}
