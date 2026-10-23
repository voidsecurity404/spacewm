//! Reload the config when the file changes.
//!
//! inotify (through the `notify` crate) runs on its own thread; it sends a message into
//! a calloop channel, so the reload itself happens on the main thread like everything
//! else. We watch the DIRECTORY, not the file: editors usually save by writing a new
//! file and renaming it over the old one, which would silently end a watch on the old
//! file's inode.

use std::{path::Path, time::Duration};

use notify::{EventKind, RecursiveMode, Watcher};
use smithay::reexports::calloop::{
    channel::{self, Event as ChannelEvent},
    timer::{TimeoutAction, Timer},
    LoopHandle,
};

use crate::state::SpaceState;

/// Editors write in several steps (truncate, write, rename); wait for the dust to settle.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Start watching `path`. Dropping the returned watcher stops it.
pub fn watch(
    path: &Path,
    loop_handle: &LoopHandle<'static, SpaceState>,
) -> anyhow::Result<notify::RecommendedWatcher> {
    let dir = path.parent().ok_or_else(|| anyhow::anyhow!("config path has no directory"))?;
    let name = path.file_name().map(|n| n.to_os_string());
    let (tx, rx) = channel::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        let relevant = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_))
            && event.paths.iter().any(|p| p.file_name().map(|n| n.to_os_string()) == name);
        if relevant {
            let _ = tx.send(());
        }
    })?;
    watcher.watch(dir, RecursiveMode::NonRecursive)?;

    loop_handle
        .insert_source(rx, |event, _, state| {
            if let ChannelEvent::Msg(()) = event {
                schedule_reload(state);
            }
        })
        .map_err(|e| anyhow::anyhow!("cannot watch the config: {}", e.error))?;
    Ok(watcher)
}

fn schedule_reload(state: &mut SpaceState) {
    if let Some(token) = state.config_reload_timer.take() {
        state.loop_handle.remove(token);
    }
    state.config_reload_timer = state
        .loop_handle
        .insert_source(Timer::from_duration(DEBOUNCE), |_, _, state| {
            state.config_reload_timer = None;
            // Errors are logged and shown on screen by reload_config itself.
            let _ = state.reload_config();
            TimeoutAction::Drop
        })
        .ok();
}
