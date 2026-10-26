//! Reopening the apps that were open when spacewm last exited.
//!
//! With the layout (where each window was) spacewm saves which programs had windows: one
//! entry per running process, so three terminals come back as three, while Firefox with
//! two windows is one process and comes back once (it restores its own windows). Only
//! the program file is kept, never its arguments: `foot -e some-command` must not run
//! that command again on the next login. The saved placements then put every window
//! back where it was.

use smithay::{desktop::Window, reexports::wayland_server::Resource};

use crate::state::SpaceState;

/// Programs that are never reopened (spacewm's own helpers, the X server).
const NEVER: &[&str] = &["spacewm", "spacectl", "Xwayland", "test_locker", "swaylock", "hyprlock"];

/// The process that owns a window: the Wayland client's pid, or for X11 windows the
/// pid the app announced (_NET_WM_PID), since the Wayland client there is Xwayland.
pub fn window_pid(state: &SpaceState, window: &Window) -> Option<u32> {
    if let Some(x11) = window.x11_surface() {
        return x11.pid();
    }
    let surface = window.toplevel()?.wl_surface().clone();
    let client = surface.client()?;
    let creds = client.get_credentials(&state.display_handle).ok()?;
    u32::try_from(creds.pid).ok()
}

/// The program file a process runs ("/usr/bin/foot").
fn exe_of(pid: u32) -> Option<String> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let exe = exe.to_string_lossy();
    // A program updated while running shows as "... (deleted)".
    Some(exe.trim_end_matches(" (deleted)").to_string())
}

fn reopenable(exe: &str) -> bool {
    let name = exe.rsplit('/').next().unwrap_or(exe);
    !NEVER.contains(&name)
}

impl SpaceState {
    /// One program per process with a window open now, in panel order.
    pub fn open_apps(&self) -> Vec<String> {
        let mut seen = Vec::new();
        let mut apps = Vec::new();
        for panel in self.world.panels() {
            let Some(pid) = window_pid(self, &panel.window) else { continue };
            if seen.contains(&pid) {
                continue;
            }
            seen.push(pid);
            if let Some(exe) = exe_of(pid).filter(|e| reopenable(e)) {
                apps.push(exe);
            }
        }
        apps
    }

    /// At startup: start what was open last time (minus what autostart starts anyway).
    pub fn restore_apps(&mut self) {
        let apps = std::mem::take(&mut self.layout.apps);
        if apps.is_empty() {
            return;
        }
        let autostart: Vec<String> = self
            .config
            .general
            .autostart
            .iter()
            .filter_map(|c| shell_words::split(c).ok()?.into_iter().next())
            .map(|p| p.rsplit('/').next().unwrap_or(&p).to_string())
            .collect();
        tracing::info!(count = apps.len(), "reopening last session's apps");
        for exe in apps {
            let name = exe.rsplit('/').next().unwrap_or(&exe).to_string();
            if autostart.contains(&name) || !reopenable(&exe) {
                continue;
            }
            if !std::path::Path::new(&exe).is_file() {
                tracing::info!(exe, "not reopened: no longer installed");
                continue;
            }
            // Open outside spacewm right now: leave it there (no questions at startup).
            let cmd = shell_words::quote(&exe).into_owned();
            if !self.outside_copies(&cmd).is_empty() {
                tracing::info!(exe, "not reopened: open outside spacewm");
                continue;
            }
            // Quote it: an odd path is still one argument.
            if let Err(e) = self.launch(&cmd) {
                tracing::warn!("reopening {exe}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_are_not_reopened() {
        assert!(reopenable("/usr/bin/foot"));
        assert!(reopenable("/usr/lib/firefox/firefox"));
        assert!(!reopenable("/usr/bin/Xwayland"));
        assert!(!reopenable("/home/u/spacewm/target/debug/spacectl"));
    }

    #[test]
    fn own_exe_is_found() {
        let me = exe_of(std::process::id()).expect("own exe");
        assert!(me.starts_with('/'));
    }
}
