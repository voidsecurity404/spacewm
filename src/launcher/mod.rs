//! The built-in launcher (Mod+Space): type part of an app's name, Enter starts it.
//! Drawn as a card: a search field, the best matches with their icons, the keys.
//!
//! A 2D box over the view. While it is open every key goes to it (walking stops); the
//! list of apps is re-read from the .desktop files each time it opens, so newly installed
//! apps show up without a restart.

pub mod desktop;
pub mod fuzzy;

use smithay::{
    backend::renderer::element::memory::MemoryRenderBuffer,
    input::keyboard::{xkb, Keysym},
};

use crate::{render::text::Line, state::SpaceState};
use desktop::DesktopEntry;

/// Results shown at once.
const VISIBLE: usize = 7;

/// Layout in logical px.
const W: i32 = 600;
const PAD: i32 = 18;
const SEARCH_H: i32 = 50;
const ROW_H: i32 = 54;
const ICON: i32 = 34;
const FOOT_H: i32 = 38;
const RADIUS: i32 = 20;
/// Room around the card for its shadow.
const MARGIN: i32 = 16;

pub struct Launcher {
    entries: Vec<DesktopEntry>,
    query: String,
    /// Indices into `entries`, best match first.
    matches: Vec<usize>,
    selected: usize,
    /// The rendered box and its size in logical px (rebuilt after every change).
    pub image: Option<(MemoryRenderBuffer, (i32, i32))>,
}

/// What a key did.
#[derive(Debug, PartialEq)]
pub enum KeyResult {
    /// Handled, the launcher stays open.
    Changed,
    Close,
    Launch(DesktopEntry),
    Ignored,
}

impl Launcher {
    pub fn new(entries: Vec<DesktopEntry>) -> Self {
        let mut l =
            Self { entries, query: String::new(), matches: Vec::new(), selected: 0, image: None };
        l.refilter();
        l
    }

    fn refilter(&mut self) {
        self.matches = fuzzy::rank(&self.query, self.entries.iter().map(|e| e.name.as_str()));
        self.selected = 0;
    }

    pub fn selected(&self) -> Option<&DesktopEntry> {
        self.matches.get(self.selected).map(|&i| &self.entries[i])
    }

    /// `sym` is the key's symbol with modifiers applied (so Shift+a is "A"), `ctrl` whether
    /// Ctrl is held.
    pub fn key(&mut self, sym: Keysym, ctrl: bool) -> KeyResult {
        match sym {
            Keysym::Escape => KeyResult::Close,
            Keysym::Return | Keysym::KP_Enter => match self.selected() {
                Some(e) => KeyResult::Launch(e.clone()),
                None => KeyResult::Changed,
            },
            Keysym::Down | Keysym::Tab => {
                if !self.matches.is_empty() {
                    self.selected = (self.selected + 1) % self.matches.len();
                }
                KeyResult::Changed
            }
            Keysym::Up | Keysym::ISO_Left_Tab => {
                if !self.matches.is_empty() {
                    self.selected = (self.selected + self.matches.len() - 1) % self.matches.len();
                }
                KeyResult::Changed
            }
            Keysym::BackSpace => {
                if ctrl {
                    self.query.clear();
                } else {
                    self.query.pop();
                }
                self.refilter();
                KeyResult::Changed
            }
            _ if ctrl && sym == Keysym::u => {
                self.query.clear();
                self.refilter();
                KeyResult::Changed
            }
            _ => {
                let text = xkb::keysym_to_utf8(sym);
                let printable: String = text.chars().filter(|c| !c.is_control()).collect();
                if printable.is_empty() || ctrl || self.query.chars().count() >= 64 {
                    return KeyResult::Ignored;
                }
                self.query.push_str(&printable);
                self.refilter();
                KeyResult::Changed
            }
        }
    }

    /// The results shown: (name, program, icon name, selected), around the selection.
    fn visible_rows(&self) -> Vec<(String, String, Option<String>, bool)> {
        let first = self.selected.saturating_sub(VISIBLE - 1);
        self.matches
            .iter()
            .enumerate()
            .skip(first)
            .take(VISIBLE)
            .map(|(pos, &i)| {
                let e = &self.entries[i];
                let program = shell_words::split(&e.exec)
                    .ok()
                    .and_then(|w| w.into_iter().next())
                    .map(|p| p.rsplit('/').next().unwrap_or(&p).to_string())
                    .unwrap_or_default();
                (e.name.clone(), program, e.icon.clone(), pos == self.selected)
            })
            .collect()
    }

    /// The lines to draw: the query, then a window of results around the selection.
    #[allow(dead_code)] // the tests read the launcher's state through it
    pub fn lines(&self) -> Vec<Line> {
        let mut lines = vec![Line {
            text: format!("> {}_", self.query),
            fg: Some([1.0, 1.0, 1.0, 1.0]),
            bg: None,
        }];
        let first = self.selected.saturating_sub(VISIBLE - 1);
        for (pos, &i) in self.matches.iter().enumerate().skip(first).take(VISIBLE) {
            let e = &self.entries[i];
            let selected = pos == self.selected;
            lines.push(Line {
                text: format!("  {}", e.name),
                fg: Some(if selected { [1.0, 1.0, 1.0, 1.0] } else { [0.82, 0.84, 0.88, 1.0] }),
                bg: selected.then_some([0.20, 0.45, 0.85, 1.0]),
            });
        }
        if self.matches.is_empty() {
            lines.push(Line {
                text: "  no matching app".into(),
                fg: Some([0.6, 0.6, 0.65, 1.0]),
                bg: None,
            });
        }
        lines
    }
}

impl SpaceState {
    pub fn toggle_launcher(&mut self) {
        if self.launcher.is_some() {
            self.close_launcher();
        } else {
            self.open_launcher();
        }
    }

    fn open_launcher(&mut self) {
        let entries = desktop::load_all();
        tracing::info!(apps = entries.len(), "launcher opened");
        self.launcher = Some(Launcher::new(entries));
        // Walking stops while typing.
        self.view.controls.clear();
        self.redraw_launcher();
    }

    pub fn close_launcher(&mut self) {
        if self.launcher.take().is_some() {
            self.queue_redraw();
        }
    }

    /// A key press while the launcher is open (`ctrl`: Ctrl held).
    pub fn launcher_key(&mut self, sym: Keysym, ctrl: bool) {
        let Some(launcher) = self.launcher.as_mut() else { return };
        match launcher.key(sym, ctrl) {
            KeyResult::Changed => self.redraw_launcher(),
            KeyResult::Ignored => {}
            KeyResult::Close => self.close_launcher(),
            KeyResult::Launch(entry) => {
                self.close_launcher();
                self.launch_entry(&entry);
            }
        }
    }

    fn launch_entry(&mut self, entry: &DesktopEntry) {
        // Terminal apps (htop, vim) run inside the configured terminal.
        let command = if entry.terminal {
            format!("{} -e {}", self.config.general.terminal, entry.exec)
        } else {
            entry.exec.clone()
        };
        tracing::info!(app = entry.name, command, "launching");
        self.expect_window_until =
            Some(std::time::Instant::now() + crate::input::mode::SPAWN_FOCUS_WINDOW);
        // Errors are shown by launch_named.
        let _ = self.launch_named(&command, &entry.name);
    }

    fn redraw_launcher(&mut self) {
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        let Some(launcher) = self.launcher.as_ref() else { return };
        let rows = launcher.visible_rows();
        let query = launcher.query.clone();
        let total = launcher.matches.len();
        let icons: Vec<Option<Vec<u8>>> = rows
            .iter()
            .map(|(_, _, icon, _)| {
                icon.as_deref().and_then(|i| self.icon_by_name(i, (ICON * scale) as u32))
            })
            .collect();
        let image = draw(&query, &rows, &icons, total, scale);
        if let Some(launcher) = self.launcher.as_mut() {
            launcher.image = image;
        }
        self.queue_redraw();
    }
}

/// The launcher card (with its shadow margin) and its size in logical px.
fn draw(
    query: &str,
    rows: &[(String, String, Option<String>, bool)],
    icons: &[Option<Vec<u8>>],
    total: usize,
    s: i32,
) -> Option<(MemoryRenderBuffer, (i32, i32))> {
    use crate::inventory::draw::Canvas;
    let list_h = rows.len().max(1) as i32 * ROW_H;
    let h = PAD + SEARCH_H + 12 + list_h + FOOT_H;
    let (cw, ch) = (W + 2 * MARGIN, h + 2 * MARGIN);
    let mut c = Canvas::new(cw * s, ch * s);
    let at = |v: i32| (v + MARGIN) * s;
    let white = [0.96, 0.97, 0.99, 1.0];
    let gray = [0.56, 0.59, 0.65, 1.0];
    let accent = [0.30, 0.55, 1.0, 1.0];
    c.shadow(at(0), at(5), W * s, h * s, RADIUS * s, MARGIN * s, 0.5);
    c.round_gradient(
        at(0),
        at(0),
        W * s,
        h * s,
        RADIUS * s,
        [0.13, 0.14, 0.18, 0.97],
        [0.08, 0.09, 0.12, 0.97],
    );
    c.round_frame(at(0), at(0), W * s, h * s, RADIUS * s, s, [1.0, 1.0, 1.0, 0.08]);

    // The search field, with a drawn magnifier.
    let (fx, fy, fw) = (PAD, PAD, W - 2 * PAD);
    c.round_rect(at(fx), at(fy), fw * s, SEARCH_H * s, 14 * s, [1.0, 1.0, 1.0, 0.06]);
    c.round_frame(at(fx), at(fy), fw * s, SEARCH_H * s, 14 * s, s, [0.40, 0.62, 1.0, 0.55]);
    let (mx, my) = (fx + 26, fy + SEARCH_H / 2 - 2);
    c.round_frame(at(mx - 8), at(my - 8), 16 * s, 16 * s, 8 * s, 2 * s, [0.75, 0.80, 0.90, 1.0]);
    for k in 0..6 * s {
        let p = at(mx + 5) + k;
        c.fill(p, p - at(mx + 5) + at(my + 5), 2 * s, 2 * s, [0.75, 0.80, 0.90, 1.0]);
    }
    let tx = at(fx + 48);
    let ty = at(fy + SEARCH_H / 2 - 12);
    if query.is_empty() {
        c.text("Search apps ...", 18.0, gray, tx, ty, s);
        c.fill(tx, ty + 2 * s, 2 * s, 22 * s, accent);
    } else {
        let (qw, _) = c.text(query, 18.0, white, tx, ty, s);
        c.fill(tx + qw + 2 * s, ty + 2 * s, 2 * s, 22 * s, accent);
    }

    // The results.
    let y0 = PAD + SEARCH_H + 12;
    if rows.is_empty() {
        let t = "No matching app";
        let w = Canvas::text_width(t, 16.0, s);
        c.text(t, 16.0, gray, at(W / 2) - w / 2, at(y0 + ROW_H / 2 - 10), s);
    }
    for (i, (name, program, _, selected)) in rows.iter().enumerate() {
        let y = y0 + i as i32 * ROW_H;
        if *selected {
            c.round_rect(
                at(PAD),
                at(y + 2),
                fw * s,
                (ROW_H - 4) * s,
                12 * s,
                [0.30, 0.55, 1.0, 0.20],
            );
            c.round_frame(
                at(PAD),
                at(y + 2),
                fw * s,
                (ROW_H - 4) * s,
                12 * s,
                s,
                [0.40, 0.62, 1.0, 0.6],
            );
        }
        let (ix, iy) = (at(PAD + 12), at(y + (ROW_H - ICON) / 2));
        match icons.get(i).and_then(|i| i.as_ref()) {
            Some(px) => c.blit(px, ICON * s, ICON * s, ix, iy),
            None => {
                // No icon: the first letter on a colored tile.
                c.round_rect(ix, iy, ICON * s, ICON * s, 9 * s, [0.28, 0.42, 0.72, 1.0]);
                let letter: String =
                    name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
                let lw = Canvas::text_width(&letter, 18.0, s);
                c.text(&letter, 18.0, white, ix + (ICON * s - lw) / 2, iy + 6 * s, s);
            }
        }
        let nx = at(PAD + 12 + ICON + 14);
        c.text(
            name,
            16.0,
            if *selected { white } else { [0.88, 0.90, 0.94, 1.0] },
            nx,
            at(y + 9),
            s,
        );
        c.text(program, 12.0, gray, nx, at(y + 31), s);
        if *selected {
            let hint = "Enter";
            let hw = Canvas::text_width(hint, 12.0, s);
            let px = at(W - PAD - 14) - hw - 12 * s;
            c.round_rect(px, at(y + 16), hw + 12 * s, 22 * s, 7 * s, [1.0, 1.0, 1.0, 0.10]);
            c.text(hint, 12.0, [0.80, 0.86, 0.96, 1.0], px + 6 * s, at(y + 19), s);
        }
    }

    // Footer.
    let fyy = h - FOOT_H + 6;
    c.fill(at(PAD), at(fyy - 6), fw * s, s.max(1), [1.0, 1.0, 1.0, 0.07]);
    c.text("↑ ↓ choose   Enter open   Esc close", 12.0, gray, at(PAD + 4), at(fyy + 4), s);
    let count = format!("{total} apps");
    let cw2 = Canvas::text_width(&count, 12.0, s);
    c.text(&count, 12.0, gray, at(W - PAD - 4) - cw2, at(fyy + 4), s);

    let buffer = MemoryRenderBuffer::from_slice(
        &c.rgba,
        smithay::backend::allocator::Fourcc::Abgr8888,
        (c.width, c.height),
        s,
        smithay::utils::Transform::Normal,
        None,
    );
    Some((buffer, (cw, ch)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> DesktopEntry {
        DesktopEntry {
            id: format!("{name}.desktop"),
            name: name.into(),
            exec: name.into(),
            terminal: false,
            icon: None,
            wm_class: None,
            terminal_emulator: false,
        }
    }

    #[test]
    fn typing_filters_and_enter_launches() {
        let mut l = Launcher::new(vec![entry("Firefox"), entry("Files"), entry("foot")]);
        assert_eq!(l.matches.len(), 3);
        for c in [Keysym::f, Keysym::f, Keysym::x] {
            assert_eq!(l.key(c, false), KeyResult::Changed);
        }
        assert_eq!(l.selected().map(|e| e.name.as_str()), Some("Firefox"));
        assert_eq!(l.key(Keysym::Return, false), KeyResult::Launch(entry("Firefox")));
    }

    #[test]
    fn arrows_wrap_and_backspace_edits() {
        let mut l = Launcher::new(vec![entry("a1"), entry("a2"), entry("a3")]);
        l.key(Keysym::Up, false);
        assert_eq!(l.selected, 2, "up from the top wraps to the bottom");
        l.key(Keysym::Down, false);
        assert_eq!(l.selected, 0);
        l.key(Keysym::a, false);
        l.key(Keysym::_2, false);
        assert_eq!(l.matches.len(), 1);
        l.key(Keysym::BackSpace, false);
        assert_eq!(l.query, "a");
        l.key(Keysym::u, true);
        assert_eq!(l.query, "", "Ctrl+U clears");
        assert_eq!(l.key(Keysym::Escape, false), KeyResult::Close);
    }

    #[test]
    fn no_match_shows_a_hint_and_enter_does_nothing() {
        let mut l = Launcher::new(vec![entry("Firefox")]);
        l.key(Keysym::z, false);
        assert!(l.lines().iter().any(|line| line.text.contains("no matching")));
        assert_eq!(l.key(Keysym::Return, false), KeyResult::Changed);
    }
}
