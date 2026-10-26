//! An app you opened went to its copy running outside spacewm: offer to bring it here.
//!
//! Noticed in launching.rs (the start exited fine without a window, see isolate.rs).
//! Then, by `desktop.running_elsewhere`: ask ("Close it there and open it here?"), do it
//! right away, or just say where it went. Moving asks the copy outside to quit (as
//! closing its window would), waits for it to be gone, and starts the app again: now
//! nothing is there to hand the request to, so it opens in spacewm.

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
}

impl SpaceState {
    /// `p` exited fine without a window: did it hand the request to a copy outside?
    pub fn check_handed_elsewhere(&mut self, p: &Pending) {
        // The programs it ran, and (if the start was too quick to see) the one it names.
        let mut names: Vec<String> = p.seen.iter().map(|(_, n)| n.clone()).collect();
        let first = shell_words::split(&p.cmd).ok().and_then(|w| w.into_iter().next());
        names.extend(first.and_then(|f| crate::isolate::program_name(&f)));
        names.sort();
        names.dedup();
        let pids = crate::isolate::running_elsewhere(&names, p.start_ticks);
        if pids.is_empty() {
            return;
        }
        tracing::info!(app = p.name, ?pids, ?names, "the app went to its copy outside spacewm");
        self.offer_move(p.name.clone(), p.cmd.clone(), pids);
    }

    fn offer_move(&mut self, app: String, cmd: String, pids: Vec<u32>) {
        match self.config.desktop.running_elsewhere {
            RunningElsewhere::Move => self.move_here(app, cmd, pids),
            RunningElsewhere::Leave => {
                self.show_info(format!(
                    "{app} is already open outside spacewm and opened its window there."
                ));
            }
            RunningElsewhere::Ask => self.open_confirm(
                format!("{app} is already open outside spacewm"),
                vec![
                    "It opened its window there, not here.".into(),
                    format!("Close {app} there and open it here?"),
                    "(It closes as if you closed its window. Logins and tabs stay.)".into(),
                ],
                "Close there, open here".into(),
                "Leave it".into(),
                ConfirmAction::MoveHere { app, cmd, pids },
            ),
        }
    }

    /// Ask the copies outside to quit; once they are gone, start the app here.
    pub fn move_here(&mut self, app: String, cmd: String, pids: Vec<u32>) {
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
        self.desktop.moving = Some(Moving { app, cmd, pids, since: Instant::now() });
        let _ = self.loop_handle.insert_source(Timer::from_duration(CHECK_EVERY), |_, _, state| {
            let Some(m) = state.desktop.moving.clone() else { return TimeoutAction::Drop };
            if m.pids.iter().any(|&pid| crate::isolate::alive(pid)) {
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
            let _ = state.launch_named(&m.cmd, &m.app);
            TimeoutAction::Drop
        });
    }
}
