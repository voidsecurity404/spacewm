//! "Starting Firefox ..." while an app the user asked for is on its way, so a slow start
//! never looks like nothing happened (and nobody clicks twice). The line goes away when
//! a window of that process (or of a child it started) opens.
//!
//! Errors are told plainly: a command that can't be started, or a program that exits
//! with an error before showing a window. No false alarms: a program that exits fine
//! without a window (it handed the request to a copy already running, or never had a
//! window) or that takes very long just drops off the list quietly.

use std::time::{Duration, Instant};

use smithay::{
    desktop::Window,
    reexports::calloop::timer::{TimeoutAction, Timer},
};

use crate::{
    inventory::{draw::Canvas, Image},
    state::SpaceState,
};

/// Fast starts (a terminal) never show the line: no flicker.
const SHOW_AFTER: Duration = Duration::from_millis(400);
/// After this the line goes away even without a window.
const GIVE_UP: Duration = Duration::from_secs(20);
/// Dots animation step.
const STEP: Duration = Duration::from_millis(350);

#[derive(Debug, Clone)]
pub struct Pending {
    pub pid: u32,
    pub name: String,
    pub since: Instant,
}

#[derive(Default)]
pub struct Launching {
    pub pending: Vec<Pending>,
    /// The rendered line and what it was rendered for (text).
    pub image: Option<Image>,
    drawn: String,
    ticking: bool,
}

/// What the user sees for a command: "firefox" for "/usr/lib/firefox/firefox --new".
pub fn display_name(cmd: &str) -> String {
    let first = shell_words::split(cmd).ok().and_then(|w| w.into_iter().next());
    let program = first.unwrap_or_else(|| cmd.to_string());
    program.rsplit('/').next().unwrap_or(&program).to_string()
}

/// The text for what is waiting: "Starting Firefox ..." / "Starting Firefox, foot ...".
pub fn line(pending: &[&str], phase: usize) -> String {
    let dots = ".".repeat(phase % 3 + 1);
    format!("Starting {} {dots:<3}", pending.join(", "))
}

fn draw(text: &str, scale: i32) -> Option<Image> {
    let s = scale;
    let style = crate::render::text::Style {
        size: 15.0,
        fg: [0.95, 0.96, 0.98, 1.0],
        bg: [0.0; 4],
        padding: 0,
        max_width: 1400,
    };
    let img = crate::render::text::render(&[text.to_string()], style, s)?;
    let (pad_x, pad_y) = (14 * s, 8 * s);
    let mut c = Canvas::new(img.width + 2 * pad_x, img.height + 2 * pad_y);
    c.fill(0, 0, c.width, c.height, [0.10, 0.22, 0.42, 0.90]);
    c.blit(&img.rgba, img.width, img.height, pad_x, pad_y);
    let size = (c.width / s, c.height / s);
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
    /// An app the user asked for was started.
    pub fn launch_started(&mut self, pid: u32, name: String) {
        self.desktop.launching.pending.push(Pending { pid, name, since: Instant::now() });
        self.schedule_launch_tick();
    }

    /// A window opened: if it belongs to a process we are waiting for (or one of its
    /// children: launcher scripts), that wait is over.
    pub fn launch_window_opened(&mut self, window: &Window) {
        if self.desktop.launching.pending.is_empty() {
            return;
        }
        let Some(pid) = crate::desktop::restore::window_pid(self, window) else { return };
        let before = self.desktop.launching.pending.len();
        self.desktop.launching.pending.retain(|p| !crate::isolate::is_descendant_of(pid, p.pid));
        if self.desktop.launching.pending.len() != before {
            self.queue_redraw();
        }
    }

    /// A started process exited (from the reaper thread).
    pub fn launch_exited(&mut self, pid: u32, code: Option<i32>) {
        let Some(i) = self.desktop.launching.pending.iter().position(|p| p.pid == pid) else {
            return;
        };
        let p = self.desktop.launching.pending.remove(i);
        self.queue_redraw();
        match code {
            Some(0) | None => {
                tracing::debug!(app = p.name, "exited without a window of its own");
            }
            Some(code) => {
                tracing::warn!(app = p.name, code, "app failed to start");
                self.show_error(vec![format!("{} could not start (exit code {code})", p.name)]);
            }
        }
    }

    /// Animate the dots while something is pending; drop entries that took too long.
    fn schedule_launch_tick(&mut self) {
        if self.desktop.launching.ticking {
            return;
        }
        self.desktop.launching.ticking = true;
        let _ = self.loop_handle.insert_source(Timer::from_duration(STEP), |_, _, state| {
            let l = &mut state.desktop.launching;
            l.pending.retain(|p| {
                let keep = p.since.elapsed() < GIVE_UP;
                if !keep {
                    tracing::info!(
                        app = p.name,
                        "no window after a long time; not waiting anymore"
                    );
                }
                keep
            });
            state.queue_redraw();
            if state.desktop.launching.pending.is_empty() {
                state.desktop.launching.ticking = false;
                TimeoutAction::Drop
            } else {
                TimeoutAction::ToDuration(STEP)
            }
        });
    }

    /// Before each frame: the "Starting ..." image for what has waited long enough.
    pub fn refresh_launching(&mut self) {
        let l = &self.desktop.launching;
        let shown: Vec<&str> = l
            .pending
            .iter()
            .filter(|p| p.since.elapsed() >= SHOW_AFTER)
            .map(|p| p.name.as_str())
            .collect();
        if shown.is_empty() {
            if l.image.is_some() {
                self.desktop.launching.image = None;
                self.desktop.launching.drawn.clear();
            }
            return;
        }
        let oldest = l.pending.iter().map(|p| p.since).min().unwrap_or_else(Instant::now);
        let phase = (oldest.elapsed().as_millis() / STEP.as_millis()) as usize;
        let text = line(&shown, phase);
        if text == l.drawn && l.image.is_some() {
            return;
        }
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        tracing::debug!(text, "launching");
        self.desktop.launching.image = draw(&text, scale);
        self.desktop.launching.drawn = text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_lines() {
        assert_eq!(display_name("/usr/lib/firefox/firefox --new-window"), "firefox");
        assert_eq!(display_name("Telegram --"), "Telegram");
        assert_eq!(display_name("'/opt/My App/app' x"), "app");
        assert_eq!(line(&["Firefox"], 0), "Starting Firefox .  ");
        assert_eq!(line(&["Firefox", "foot"], 2), "Starting Firefox, foot ...");
        // The width never changes while the dots move: no jumping box.
        assert_eq!(line(&["x"], 0).len(), line(&["x"], 1).len());
    }
}
