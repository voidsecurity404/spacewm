//! An app you open from spacewm must never open its window outside spacewm.
//!
//! Single-instance apps hand a second start to the copy already running (isolate.rs);
//! if that copy is outside spacewm, the window would open there. So before starting an
//! app, spacewm looks for a copy of it running outside (by program, plus the programs a
//! launcher script was seen handing off to). Found: it is NOT started. Instead, by
//! `desktop.running_elsewhere`, spacewm asks ("Close it there and open it here?") or does
//! that right away. Moving asks the copy outside to quit (as closing its window would),
//! waits until every process of it is gone, and only then starts the app: with nothing
//! to hand the request to, it opens in spacewm. Nothing ever opens outside first.
//!
//! Terminals (Categories=TerminalEmulator in their .desktop file, and your configured
//! terminal) run many copies side by side: they are never stopped or closed.
//!
//! A start that still went outside (a script with another name than the program it
//! runs, the first time) is noticed afterwards (launching.rs), offered the same move,
//! and learned, so the next start is stopped before.

use std::time::{Duration, Instant};

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use super::launching::Pending;
use crate::{config::RunningElsewhere, render::confirm::ConfirmAction, state::SpaceState};

/// How often to look whether the copy outside has quit, and how long to wait for it
/// (it may be saving, or asking "save changes?" over there).
const CHECK_EVERY: Duration = Duration::from_millis(200);
const WAIT_AT_MOST: Duration = Duration::from_secs(30);

/// A copy outside being closed so the app can open here.
#[derive(Debug, Clone)]
pub struct Moving {
    pub app: String,
    pub cmd: String,
    pub pids: Vec<u32>,
    pub since: Instant,
    /// Where to start it (the folder it was typed in).
    pub cwd: Option<std::path::PathBuf>,
}

impl SpaceState {
    /// The programs a command runs as: its own, and those it was seen handing off to.
    fn program_names(cmd: &str) -> Option<(String, Vec<String>)> {
        let first = shell_words::split(cmd).ok()?.into_iter().next()?;
        let program = crate::isolate::program_name(&first)?;
        let mut names = vec![program.clone()];
        names.extend(crate::isolate::learned(&program));
        Some((program, names))
    }

    /// A terminal: many copies run side by side, none is ever closed for another.
    fn is_terminal(&mut self, program: &str) -> bool {
        let configured = shell_words::split(&self.config.general.terminal).ok();
        let configured = configured.and_then(|w| w.into_iter().next());
        if configured.and_then(|t| crate::isolate::program_name(&t)).as_deref() == Some(program) {
            return true;
        }
        self.ensure_entries();
        self.dock.entries.as_deref().unwrap_or(&[]).iter().any(|e| {
            e.terminal_emulator
                && shell_words::split(&e.exec)
                    .ok()
                    .and_then(|w| w.into_iter().next())
                    .and_then(|p| crate::isolate::program_name(&p))
                    .as_deref()
                    == Some(program)
        })
    }

    /// Copies of what `cmd` runs that are open outside spacewm (none for terminals).
    pub fn outside_copies(&mut self, cmd: &str) -> Vec<u32> {
        // `flatpak run APP`: the same app in a sandbox started outside.
        let words = shell_words::split(cmd).unwrap_or_default();
        if words.first().and_then(|w| w.rsplit('/').next()) == Some("flatpak") {
            return crate::isolate::flatpak_app(&words)
                .map(crate::isolate::flatpak_elsewhere)
                .unwrap_or_default();
        }
        let Some((program, names)) = Self::program_names(cmd) else { return Vec::new() };
        if self.is_terminal(&program) {
            return Vec::new();
        }
        crate::isolate::running_elsewhere(&names, crate::isolate::now_ticks())
    }

    /// Before starting `cmd`: if it is open outside spacewm, don't start it (it would open
    /// there); ask to move it instead. Returns true when the start was held back.
    pub fn hold_back_launch(
        &mut self,
        cmd: &str,
        app: &str,
        cwd: Option<std::path::PathBuf>,
    ) -> bool {
        if self.config.desktop.running_elsewhere == RunningElsewhere::Leave {
            return false;
        }
        let pids = self.outside_copies(cmd);
        if pids.is_empty() {
            return false;
        }
        tracing::info!(
            app,
            ?pids,
            "already open outside spacewm: not started, it would open there"
        );
        if self.confirm.is_some() || self.desktop.moving.is_some() {
            self.show_info(format!("{app} is open outside spacewm: answer the question first."));
            return true;
        }
        self.offer_move(app.to_string(), cmd.to_string(), pids, true, cwd);
        true
    }

    /// `p` exited fine without a window: did it hand the request to a copy outside?
    pub fn check_handed_elsewhere(&mut self, p: &Pending) {
        // The programs it ran, and (if the start was too quick to see) the one it names.
        let mut names: Vec<String> = p.seen.iter().map(|(_, n)| n.clone()).collect();
        let first = shell_words::split(&p.cmd).ok().and_then(|w| w.into_iter().next());
        names.extend(first.as_deref().and_then(crate::isolate::program_name));
        names.sort();
        names.dedup();
        let pids = crate::isolate::running_elsewhere(&names, p.start_ticks);
        if pids.is_empty() {
            return;
        }
        tracing::info!(app = p.name, ?pids, ?names, "the app went to its copy outside spacewm");
        // Learn which programs this start went to, so next time it is held back before.
        if let Some(program) = first.as_deref().and_then(crate::isolate::program_name) {
            let outside: Vec<String> = names
                .iter()
                .filter(|n| {
                    !crate::isolate::running_elsewhere(std::slice::from_ref(*n), p.start_ticks)
                        .is_empty()
                })
                .cloned()
                .collect();
            crate::isolate::learn(&program, &outside);
        }
        self.offer_move(p.name.clone(), p.cmd.clone(), pids, false, None);
    }

    /// `before`: asked before starting it (nothing opened outside); else afterwards.
    fn offer_move(
        &mut self,
        app: String,
        cmd: String,
        pids: Vec<u32>,
        before: bool,
        cwd: Option<std::path::PathBuf>,
    ) {
        let first_line = if before {
            "It runs only once: started here, its window would open there.".to_string()
        } else {
            "It opened its window there, not here.".to_string()
        };
        match self.config.desktop.running_elsewhere {
            RunningElsewhere::Move => self.move_here(app, cmd, pids, cwd),
            RunningElsewhere::Leave => {
                self.show_info(format!(
                    "{app} is already open outside spacewm and opened its window there."
                ));
            }
            RunningElsewhere::Ask => self.open_confirm(
                format!("{app} is already open outside spacewm"),
                vec![
                    first_line,
                    format!("Close {app} there and open it here?"),
                    "(It closes as if you closed its window. Logins and tabs stay.)".into(),
                ],
                "Close there, open here".into(),
                if before { "Cancel".into() } else { "Leave it".into() },
                ConfirmAction::MoveHere { app, cmd, pids, cwd },
            ),
        }
    }

    /// Ask the copies outside to quit; once they are gone, start the app here.
    pub fn move_here(
        &mut self,
        app: String,
        cmd: String,
        pids: Vec<u32>,
        cwd: Option<std::path::PathBuf>,
    ) {
        if self.desktop.moving.is_some() {
            return; // one at a time
        }
        for &pid in &pids {
            crate::isolate::ask_to_quit(pid);
        }
        self.show_info_for(
            format!("Closing {app} outside spacewm, then opening it here ..."),
            WAIT_AT_MOST.as_secs(),
        );
        self.desktop.moving = Some(Moving { app, cmd, pids, since: Instant::now(), cwd });
        let _ = self.loop_handle.insert_source(Timer::from_duration(CHECK_EVERY), |_, _, state| {
            let Some(m) = state.desktop.moving.clone() else { return TimeoutAction::Drop };
            // Gone means all of it: a browser's helper processes linger a moment.
            let left = m.pids.iter().any(|&pid| crate::isolate::alive(pid))
                || !state.outside_copies(&m.cmd).is_empty();
            if left {
                if m.since.elapsed() < WAIT_AT_MOST {
                    return TimeoutAction::ToDuration(CHECK_EVERY);
                }
                state.desktop.moving = None;
                tracing::warn!(app = m.app, "the copy outside spacewm did not quit");
                state.show_error(vec![
                    format!("{} did not close outside spacewm.", m.app),
                    "It may be asking something there: close it there, then open it here.".into(),
                ]);
                return TimeoutAction::Drop;
            }
            state.desktop.moving = None;
            tracing::info!(app = m.app, "closed outside spacewm; opening it here");
            state.clear_notice();
            state.expect_window_until =
                Some(Instant::now() + crate::input::mode::SPAWN_FOCUS_WINDOW);
            let _ = state.launch_named_in(&m.cmd, &m.app, m.cwd);
            TimeoutAction::Drop
        });
    }
}
