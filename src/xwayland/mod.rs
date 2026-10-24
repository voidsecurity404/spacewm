//! XWayland: running old X11 applications.
//!
//! XWayland is a real X server that is itself a Wayland client. Every X11 window it
//! manages becomes a wl_surface of the XWayland client, and our X11 window manager
//! (smithay's `X11Wm`, an ordinary X11 client connection) tells us which X11 window owns
//! which surface. So X11 windows end up as normal `Window`s and panels.
//!
//! Lazy start: Xwayland costs ~30 MB and some startup time, and most sessions never run
//! an X11 app. So at startup we only create the X11 display sockets ourselves
//! (`/tmp/.X11-unix/X<n>` plus the Linux abstract socket of the same name) and set
//! DISPLAY for our children. When the first X client connects, the socket becomes
//! readable and we launch Xwayland, handing it our listening sockets with `-listenfd`, so
//! it accepts the connection that is already waiting. Once started it keeps running: it
//! is NOT stopped when no X11 window is left, because apps (Qt, Electron) often keep an X
//! connection without any window, and taking the server away from under them freezes
//! them. If Xwayland crashes, the next X client starts a fresh server.

mod wm;

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::fs::OpenOptionsExt,
    },
    path::PathBuf,
    process::Stdio,
    time::Duration,
};

use rustix::{
    io::FdFlags,
    net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType},
};
use smithay::{
    reexports::{
        calloop::{
            generic::{FdWrapper, Generic},
            timer::{TimeoutAction, Timer},
            Interest, LoopHandle, Mode, PostAction, RegistrationToken,
        },
        wayland_server::{Client, DisplayHandle},
    },
    utils::{Point, Size},
    xwayland::{X11Wm, XWayland, XWaylandEvent},
};

use crate::state::SpaceState;

pub use wm::X11Child;

/// Failed starts in a row before X11 support is switched off for this session.
const MAX_FAILURES: u32 = 5;

/// X displays we try, like other compositors: the first free number from 1 (`:0` is
/// traditionally the "real" X server, which may still exist on some systems).
const DISPLAYS: std::ops::RangeInclusive<u32> = 1..=32;

/// Our X11 display: sockets we own, and the running server if any.
pub struct Xwayland {
    pub display: u32,
    lock_path: PathBuf,
    socket_path: PathBuf,
    /// Listening sockets: [filesystem, abstract]. Kept for the whole run.
    listeners: Vec<OwnedFd>,
    /// Calloop sources watching the listeners while no server runs.
    watches: Vec<RegistrationToken>,
    server: Option<Server>,
    /// The X11 window manager connection, once the server is ready.
    pub wm: Option<X11Wm>,
    /// Window managers of servers we stopped. Their event source can still deliver
    /// queued X events, and every one of those asks `xwm_state` for its wm, so a wm is
    /// only dropped on its source's final `disconnected` callback.
    pub retired: Vec<X11Wm>,
    /// Times the server was started (for the log).
    starts: u32,
    /// Starts in a row that failed before the server became ready.
    failures: u32,
}

/// A running (or starting) Xwayland process.
struct Server {
    /// The `XWayland` event source. Removing it drops the handle, which kills the
    /// XWayland client, so it stays registered as long as the server runs.
    token: RegistrationToken,
    client: Client,
}

impl Xwayland {
    /// Create the display sockets and wait for the first X client. Errors (no free
    /// display, /tmp not writable) only disable X11 support.
    pub fn init(loop_handle: &LoopHandle<'static, SpaceState>) -> anyhow::Result<Self> {
        let mut last_err = None;
        for display in DISPLAYS {
            match Self::bind(display) {
                Ok(mut xw) => {
                    xw.arm(loop_handle)?;
                    let name = format!(":{display}");
                    tracing::info!(display = %name, "X11 display ready (lazy)");
                    return Ok(xw);
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no free X11 display")))
    }

    fn bind(display: u32) -> anyhow::Result<Self> {
        // The lock file is how X servers agree on who owns a display number: whoever
        // creates it first (O_EXCL) wins. It holds the owner's pid, right-aligned in 10
        // characters, which is the format other X tools expect.
        let lock_path = PathBuf::from(format!("/tmp/.X{display}-lock"));
        let create =
            || OpenOptions::new().write(true).create_new(true).mode(0o444).open(&lock_path);
        let mut lock = match create() {
            Ok(lock) => lock,
            // A lock whose owner is dead was left by a crash (ours or another server's):
            // take it over, like X servers do.
            Err(e)
                if e.kind() == std::io::ErrorKind::AlreadyExists && lock_is_stale(&lock_path) =>
            {
                tracing::info!(lock = %lock_path.display(), "removing stale X11 lock");
                fs::remove_file(&lock_path)?;
                create()?
            }
            Err(e) => return Err(anyhow::anyhow!("{}: {e}", lock_path.display())),
        };
        let result = (|| {
            writeln!(lock, "{:>10}", std::process::id())?;
            fs::create_dir_all("/tmp/.X11-unix")?;
            let socket_path = PathBuf::from(format!("/tmp/.X11-unix/X{display}"));
            // We own the lock, so a socket file left over from a crashed server is stale.
            let _ = fs::remove_file(&socket_path);
            let fs_addr = SocketAddrUnix::new(&socket_path)?;
            let abstract_name = format!("/tmp/.X11-unix/X{display}");
            let abs_addr = SocketAddrUnix::new_abstract_name(abstract_name.as_bytes())?;
            let listeners = vec![listen(&fs_addr)?, listen(&abs_addr)?];
            Ok::<_, anyhow::Error>((socket_path, listeners))
        })();
        match result {
            Ok((socket_path, listeners)) => Ok(Self {
                display,
                lock_path,
                socket_path,
                listeners,
                watches: Vec::new(),
                server: None,
                wm: None,
                retired: Vec::new(),
                starts: 0,
                failures: 0,
            }),
            Err(e) => {
                let _ = fs::remove_file(&lock_path);
                Err(e)
            }
        }
    }

    /// Watch the listening sockets: a pending connection makes them readable.
    fn arm(&mut self, loop_handle: &LoopHandle<'static, SpaceState>) -> anyhow::Result<()> {
        if !self.watches.is_empty() {
            for token in &self.watches {
                loop_handle.enable(token).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            return Ok(());
        }
        for (index, fd) in self.listeners.iter().enumerate() {
            // SAFETY: the listener fds live in `self` for the whole run and are removed
            // from the loop (shutdown) before being closed.
            let wrapper = unsafe { FdWrapper::new(fd.as_raw_fd()) };
            let token = loop_handle
                .insert_source(
                    Generic::new(wrapper, Interest::READ, Mode::Level),
                    move |_, _, state| {
                        start_server(state, index);
                        // Stop watching until this server exits (see `server_gone`).
                        Ok(PostAction::Disable)
                    },
                )
                .map_err(|e| anyhow::anyhow!("cannot watch X11 socket: {}", e.error))?;
            self.watches.push(token);
        }
        Ok(())
    }

    /// Remove sockets and the lock file (clean exit).
    pub fn shutdown(&mut self, loop_handle: &LoopHandle<'static, SpaceState>) {
        for token in self.watches.drain(..) {
            loop_handle.remove(token);
        }
        if let Some(server) = self.server.take() {
            loop_handle.remove(server.token);
        }
        self.wm = None;
        self.retired.clear();
        self.listeners.clear();
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// The lock file names a pid that no longer exists.
fn lock_is_stale(path: &std::path::Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else { return false };
    match text.trim().parse::<u32>() {
        // kill(pid, 0) would also work, but fails for other users' processes; /proc
        // existence is what we need: is ANY process still holding this display?
        Ok(pid) if pid > 0 => !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        _ => false,
    }
}

fn listen(addr: &SocketAddrUnix) -> anyhow::Result<OwnedFd> {
    let fd = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )?;
    rustix::net::bind(&fd, addr)?;
    rustix::net::listen(&fd, 32)?;
    Ok(fd)
}

/// The first X client is waiting: launch Xwayland on our sockets.
/// `ready` is the index of the listener that woke us; it disables itself, the other
/// one we disable here so that `arm` can enable both again later.
fn start_server(state: &mut SpaceState, ready: usize) {
    let Some(xw) = state.xwayland.as_mut() else { return };
    for (i, token) in xw.watches.iter().enumerate() {
        if i != ready {
            let _ = state.loop_handle.disable(token);
        }
    }
    if xw.server.is_some() {
        return;
    }
    xw.starts += 1;
    tracing::info!(
        display = xw.display,
        start = xw.starts,
        "X client connecting, starting Xwayland"
    );

    // Xwayland must inherit the listening sockets, so clear CLOEXEC just for this spawn
    // and set it back right after (otherwise every app we start would inherit them).
    let fds: Vec<_> = xw.listeners.iter().map(|fd| fd.as_fd()).collect();
    let mut args = Vec::new();
    for fd in &fds {
        let _ = rustix::io::fcntl_setfd(fd, FdFlags::empty());
        args.push("-listenfd".to_string());
        args.push(fd.as_raw_fd().to_string());
    }
    let spawned = XWayland::spawn(
        &state.display_handle,
        None,
        std::iter::empty::<(String, String)>(),
        args,
        false,
        Stdio::null(),
        Stdio::null(),
        |_| (),
    );
    for fd in &fds {
        let _ = rustix::io::fcntl_setfd(fd, FdFlags::CLOEXEC);
    }

    let (xwayland, client) = match spawned {
        Ok(x) => x,
        Err(e) => {
            tracing::error!("cannot start Xwayland (is it installed?): {e}");
            start_failed(state);
            return;
        }
    };
    let dh = state.display_handle.clone();
    let inserted = state.loop_handle.insert_source(xwayland, move |event, _, state| match event {
        XWaylandEvent::Ready { x11_socket, display_number } => {
            on_ready(state, &dh, x11_socket, display_number)
        }
        XWaylandEvent::Error => {
            tracing::error!("Xwayland exited during startup");
            start_failed(state);
        }
    });
    match inserted {
        Ok(token) => {
            if let Some(xw) = state.xwayland.as_mut() {
                xw.server = Some(Server { token, client });
            }
        }
        Err(e) => {
            tracing::error!("cannot watch Xwayland: {}", e.error);
            start_failed(state);
        }
    }
}

fn on_ready(
    state: &mut SpaceState,
    dh: &DisplayHandle,
    x11_socket: std::os::unix::net::UnixStream,
    display_number: u32,
) {
    let Some(client) =
        state.xwayland.as_ref().and_then(|x| x.server.as_ref()).map(|s| s.client.clone())
    else {
        return;
    };
    match X11Wm::start_wm(state.loop_handle.clone(), dh, x11_socket, client) {
        Ok(mut wm) => {
            // The root window's cursor: X apps that don't set their own cursor show this.
            let (pixels, size, hotspot) = state.cursor_theme.default_image();
            if let Err(e) = wm.set_cursor(
                &pixels,
                Size::from((size.0 as u16, size.1 as u16)),
                Point::from((hotspot.0 as u16, hotspot.1 as u16)),
            ) {
                tracing::debug!("cannot set X11 cursor: {e}");
            }
            tracing::info!(internal_display = display_number, "Xwayland ready");
            if let Some(xw) = state.xwayland.as_mut() {
                xw.wm = Some(wm);
                xw.failures = 0;
            }
        }
        Err(e) => {
            tracing::error!("cannot start the X11 window manager: {e}");
            start_failed(state);
        }
    }
}

/// The server exited (last X client closed, or a crash): forget it, drop its windows and
/// wait for the next X client.
pub fn server_gone(state: &mut SpaceState) {
    let Some(xw) = state.xwayland.as_mut() else { return };
    if let Some(wm) = xw.wm.take() {
        xw.retired.push(wm);
    }
    if let Some(server) = xw.server.take() {
        state.loop_handle.remove(server.token);
    }
    // Windows of a dead server can't be closed politely; remove their panels now.
    let dead: Vec<_> =
        state.space.elements().filter(|w| w.x11_surface().is_some()).cloned().collect();
    for window in dead {
        state.remove_window(&window);
    }
    tracing::info!("Xwayland stopped; it starts again with the next X client");
    rearm_after(state, Duration::ZERO);
}

/// Xwayland didn't come up. The waiting X client keeps the socket readable, so retrying
/// at once would spin: wait a little, and give up after a few failures in a row.
fn start_failed(state: &mut SpaceState) {
    let Some(xw) = state.xwayland.as_mut() else { return };
    if let Some(wm) = xw.wm.take() {
        xw.retired.push(wm);
    }
    if let Some(server) = xw.server.take() {
        state.loop_handle.remove(server.token);
    }
    xw.failures += 1;
    if xw.failures >= MAX_FAILURES {
        tracing::error!(failures = xw.failures, "Xwayland keeps failing; X11 apps disabled");
        return;
    }
    rearm_after(state, Duration::from_secs(2));
}

fn rearm_after(state: &mut SpaceState, delay: Duration) {
    // From a timer, never inline: we may be inside the very source we want to re-enable.
    let inserted = state.loop_handle.insert_source(Timer::from_duration(delay), |_, _, state| {
        let handle = state.loop_handle.clone();
        if let Some(xw) = state.xwayland.as_mut() {
            if let Err(e) = xw.arm(&handle) {
                tracing::error!("X11 support disabled: {e}");
            }
        }
        TimeoutAction::Drop
    });
    if let Err(e) = inserted {
        tracing::error!("X11 support disabled: {}", e.error);
    }
}
