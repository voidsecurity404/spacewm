//! Starting applications inside spacewm.

use std::{
    ffi::OsStr,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        Mutex,
    },
};

use crate::state::SpaceState;

/// Our X11 display number + 1 (0: no X11). Process-wide like an environment variable,
/// but without the unsafety of changing our own environment once threads exist.
static X_DISPLAY: AtomicU32 = AtomicU32::new(0);

/// Children started from now on get DISPLAY=:n (our lazy XWayland display).
pub fn set_x_display(display: Option<u32>) {
    X_DISPLAY.store(display.map_or(0, |d| d + 1), Ordering::Relaxed);
}

fn x_display() -> Option<String> {
    match X_DISPLAY.load(Ordering::Relaxed) {
        0 => None,
        n => Some(format!(":{}", n - 1)),
    }
}

/// Where the reaper threads report exits (desktop/launching.rs waits for windows).
static EXITS: Mutex<Option<smithay::reexports::calloop::channel::Sender<crate::desktop::Msg>>> =
    Mutex::new(None);

pub fn set_exit_sender(tx: smithay::reexports::calloop::channel::Sender<crate::desktop::Msg>) {
    if let Ok(mut s) = EXITS.lock() {
        *s = Some(tx);
    }
}

/// The IPC socket, exported to children as SPACEWM_SOCKET (spacectl finds spacewm through it).
static IPC_SOCKET: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_ipc_socket(path: Option<PathBuf>) {
    if let Ok(mut s) = IPC_SOCKET.lock() {
        *s = path;
    }
}

/// The environment our children need: our Wayland socket, our X display (or none, so
/// they never reach a host X server), our IPC socket.
fn session_env(command: &mut Command, socket: &OsStr) {
    command.env("WAYLAND_DISPLAY", socket);
    match x_display() {
        Some(display) => command.env("DISPLAY", display),
        None => command.env_remove("DISPLAY"),
    };
    match IPC_SOCKET.lock().ok().and_then(|s| s.clone()) {
        Some(path) => command.env("SPACEWM_SOCKET", path),
        None => command.env_remove("SPACEWM_SOCKET"),
    };
}

/// A `Command` for a helper tool (wpctl, grim...) with our session environment and a
/// clean signal mask. Safe to build on any thread.
pub fn session_command(program: &str, socket: &OsStr) -> Command {
    let mut command = Command::new(program);
    session_env(&mut command, socket);
    // SAFETY: only async-signal-safe libc calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            // Our blocked SIGINT/SIGTERM would be inherited: the tool (or a daemon it
            // leaves running, like wl-copy) could then never be stopped.
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            Ok(())
        });
    }
    command
}

/// Run `cmd` (split like a shell would, but without a shell) as a client of our socket.
/// Errors are logged, never fatal.
pub fn spawn(cmd: &str, socket: &OsStr) -> Result<u32, String> {
    let args = shell_words::split(cmd).map_err(|e| format!("bad command '{cmd}': {e}"))?;
    spawn_args(&args, cmd, socket)
}

/// Run `cmd` through `sh -c` (pipes, redirections, `&&`). Only for callers the user
/// allowed to (config `general.ipc_allow_shell`).
pub fn spawn_shell(cmd: &str, socket: &OsStr) -> Result<u32, String> {
    spawn_args(&["sh".into(), "-c".into(), cmd.into()], cmd, socket)
}

fn spawn_args(args: &[String], label: &str, socket: &OsStr) -> Result<u32, String> {
    let (program, rest) = args.split_first().ok_or_else(|| "empty command".to_string())?;
    let mut command = Command::new(program);
    command.args(rest).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    session_env(&mut command, socket);
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
    tracing::info!(pid, command = label, args = ?args, "spawned");
    // Reap the child when it exits so it doesn't stay a zombie.
    let exits = EXITS.lock().ok().and_then(|s| s.clone());
    std::thread::spawn(move || {
        if let Ok(status) = child.wait() {
            tracing::debug!(pid, %status, "child exited");
            if let Some(tx) = exits {
                let _ = tx.send(crate::desktop::Msg::Exited { pid, code: status.code() });
            }
        }
    });
    Ok(pid)
}

fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

pub fn spawn_terminal(state: &mut SpaceState) {
    // The configured terminal, or the first common one installed if it isn't.
    let configured = state.config.general.terminal.clone();
    let program = shell_words::split(&configured).ok().and_then(|w| w.into_iter().next());
    let terminal = if program.as_deref().is_some_and(in_path) {
        configured
    } else {
        let fallback = ["foot", "weston-terminal", "alacritty", "kitty", "xterm"]
            .into_iter()
            .find(|t| in_path(t))
            .unwrap_or("foot");
        tracing::warn!(configured, fallback, "terminal not found, using another one");
        fallback.to_string()
    };
    state.expect_window_until =
        Some(std::time::Instant::now() + crate::input::mode::SPAWN_FOCUS_WINDOW);
    let _ = state.launch(&terminal);
}

impl SpaceState {
    /// Start an app the user asked for (launcher, keybind, last session's apps): shown
    /// as "Starting ..." until its window opens; failures are shown on screen.
    pub fn launch(&mut self, cmd: &str) -> Result<u32, String> {
        let name = crate::desktop::launching::display_name(cmd);
        self.launch_named(cmd, &name)
    }

    /// `launch` with the name the user knows the app by ("Visual Studio Code").
    /// If the app is open outside spacewm it is not started (its window would open there):
    /// a question offers to move it here instead, and Ok(0) is returned.
    pub fn launch_named(&mut self, cmd: &str, name: &str) -> Result<u32, String> {
        if self.hold_back_launch(cmd, name) {
            return Ok(0);
        }
        match self.launch_untracked(cmd) {
            Ok(pid) => {
                self.launch_started(pid, name.to_string(), cmd.to_string());
                Ok(pid)
            }
            Err(e) => {
                tracing::warn!("{e}");
                self.show_error(vec![format!("Cannot start {name}:"), e.clone()]);
                Err(e)
            }
        }
    }

    /// Start a program without waiting for a window (autostart daemons, the locker,
    /// IPC scripts).
    pub fn launch_untracked(&mut self, cmd: &str) -> Result<u32, String> {
        spawn(cmd, &self.socket_name)
    }
}

/// Export our session variables to the D-Bus activation environment and systemd --user,
/// so services started on demand (xdg-desktop-portal, mako, pipewire...) connect to
/// spacewm. Only meaningful on a real session (DRM): nested, they already have a desktop.
pub fn update_activation_environment(socket: &OsStr) {
    if !in_path("dbus-update-activation-environment") {
        tracing::warn!("dbus-update-activation-environment not found; portals may not work");
        return;
    }
    let mut command = Command::new("dbus-update-activation-environment");
    command
        .args(["--systemd", "WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"])
        .env("WAYLAND_DISPLAY", socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(display) = x_display() {
        command.arg("DISPLAY").env("DISPLAY", display);
    }
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
