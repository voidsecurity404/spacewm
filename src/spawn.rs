//! Starting applications inside spacewm.

use std::{
    ffi::OsStr,
    os::unix::process::CommandExt,
    process::{Command, Stdio},
};

use crate::state::SpaceState;

/// Run `cmd` (split like a shell would, but without a shell) as a client of our socket.
/// Errors are logged, never fatal.
pub fn spawn(cmd: &str, socket: &OsStr) -> Result<u32, String> {
    let args = shell_words::split(cmd).map_err(|e| format!("bad command '{cmd}': {e}"))?;
    let (program, rest) = args.split_first().ok_or_else(|| "empty command".to_string())?;
    let mut command = Command::new(program);
    command
        .args(rest)
        .env("WAYLAND_DISPLAY", socket)
        // No X11 until XWayland exists (Stage 7.1): don't let apps reach the host X server.
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: only async-signal-safe libc calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            // We block SIGINT/SIGTERM to receive them through signalfd (calloop). The
            // blocked mask is inherited across exec, so without this reset our children
            // could never be stopped with Ctrl+C or `kill`.
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            // Own session: a child doesn't die with our controlling terminal.
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|e| format!("failed to start '{program}': {e}"))?;
    let pid = child.id();
    tracing::info!(pid, command = cmd, "spawned");
    // Reap the child when it exits so it doesn't stay a zombie.
    std::thread::spawn(move || {
        if let Ok(status) = child.wait() {
            tracing::debug!(pid, %status, "child exited");
        }
    });
    Ok(pid)
}

fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

pub fn spawn_terminal(state: &mut SpaceState) {
    let terminal = ["foot", "weston-terminal", "alacritty", "kitty"]
        .into_iter()
        .find(|t| in_path(t))
        .unwrap_or("foot");
    state.expect_window_until =
        Some(std::time::Instant::now() + crate::input::mode::SPAWN_FOCUS_WINDOW);
    if let Err(err) = spawn(terminal, &state.socket_name) {
        tracing::warn!("{err}");
    }
}

/// Export our session variables to the D-Bus activation environment and systemd --user,
/// so services started on demand (xdg-desktop-portal, mako, pipewire...) connect to
/// spacewm. Only meaningful on a real session (DRM): nested, they already have a desktop.
pub fn update_activation_environment() {
    if !in_path("dbus-update-activation-environment") {
        tracing::warn!("dbus-update-activation-environment not found; portals may not work");
        return;
    }
    let mut command = Command::new("dbus-update-activation-environment");
    command
        .args(["--systemd", "WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Set for the child only: changing our own environment is unsafe once threads exist.
    if std::env::var_os("XDG_CURRENT_DESKTOP").is_none() {
        command.env("XDG_CURRENT_DESKTOP", "spacewm");
    }
    if std::env::var_os("XDG_SESSION_TYPE").is_none() {
        command.env("XDG_SESSION_TYPE", "wayland");
    }
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => tracing::warn!("failed to update activation environment: {err}"),
    }
}
