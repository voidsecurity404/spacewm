//! The built-in launcher (Mod+Space): type part of an app's name, Enter starts it.
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

use crate::{
    render::text::{self, Line, Style},
    state::SpaceState,
};
use desktop::DesktopEntry;

/// Results shown at once.
const VISIBLE: usize = 10;
/// Width of the box in characters.
const WIDTH_COLS: usize = 52;

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

    /// The lines to draw: the query, then a window of results around the selection.
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
        if let Err(e) = self.launch(&command) {
            tracing::warn!("{e}");
            self.show_error(vec![format!("Cannot start {}:", entry.name), e]);
        }
    }

    fn redraw_launcher(&mut self) {
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1);
        let Some(launcher) = self.launcher.as_mut() else { return };
        let style = Style {
            size: 20.0,
            fg: [1.0, 1.0, 1.0, 1.0],
            bg: [0.09, 0.10, 0.13, 0.94],
            padding: 14,
            max_width: 1200,
        };
        launcher.image = text::render_lines(&launcher.lines(), style, scale, WIDTH_COLS)
            .map(|img| (img.buffer(), (img.width / img.scale, img.height / img.scale)));
        self.queue_redraw();
    }
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
