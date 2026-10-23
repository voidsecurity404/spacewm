//! IPC control socket (used by `spacectl`).
//!
//! A unix socket at `$XDG_RUNTIME_DIR/spacewm-$WAYLAND_DISPLAY.sock`, mode 0600. Every
//! connection speaks newline-delimited JSON (see proto.rs).
//!
//! Security, because this socket can spawn programs and (optionally) inject input:
//! - The peer's UID is checked with SO_PEERCRED; other users are rejected even if file
//!   permissions were somehow loosened.
//! - Requests are capped at 64 KiB, connections at 16, unread output at 1 MiB; a client
//!   breaking these is dropped. Malformed input gets an error response, never a crash.
//! - All socket I/O is non-blocking, so a stuck client can't freeze the compositor.

pub mod proto;

use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use smithay::reexports::calloop::{
    channel::Sender, generic::Generic, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
};
use smithay::wayland::seat::WaylandFocus;

use crate::{
    input::inject::{self, PressAction},
    render::screenshot::{self, CapturedImage, ScreenshotDone, ScreenshotRequest},
    state::SpaceState,
};
use proto::{Event, EventKind, InputAction, MouseButton, PanelInfo, PressMode, Request, Response};

pub type ConnectionId = u64;

const MAX_CONNECTIONS: usize = 16;
const MAX_PENDING_OUTPUT: usize = 1024 * 1024;

struct Connection {
    stream: UnixStream,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    read_token: Option<RegistrationToken>,
    write_token: Option<RegistrationToken>,
    subscriptions: Vec<EventKind>,
}

pub struct IpcServer {
    pub path: PathBuf,
    connections: HashMap<ConnectionId, Connection>,
    next_id: ConnectionId,
    listener_token: Option<RegistrationToken>,
    pub allow_input_injection: bool,
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn default_socket_path(wayland_display: &std::ffi::OsStr) -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let mut name = std::ffi::OsString::from("spacewm-");
    name.push(wayland_display);
    name.push(".sock");
    Some(PathBuf::from(runtime).join(name))
}

impl IpcServer {
    pub fn start(
        path: PathBuf,
        allow_input_injection: bool,
        loop_handle: &LoopHandle<'static, SpaceState>,
    ) -> anyhow::Result<Self> {
        if path.exists() {
            // A live spacewm answers; a stale file from a crash doesn't.
            if UnixStream::connect(&path).is_ok() {
                anyhow::bail!("{} is in use by another spacewm", path.display());
            }
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        let token = loop_handle
            .insert_source(
                Generic::new(listener, Interest::READ, Mode::Level),
                |_, listener, state| {
                    loop {
                        match listener.accept() {
                            Ok((stream, _)) => accept(state, stream),
                            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                            Err(e) => {
                                tracing::warn!("ipc accept failed: {e}");
                                break;
                            }
                        }
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| anyhow::anyhow!("ipc listener source: {}", e.error))?;

        // Child processes (and spacectl run from them) find us through this.
        // SAFETY: single-threaded at this point of startup.
        unsafe { std::env::set_var("SPACEWM_SOCKET", &path) };
        tracing::info!(path = %path.display(), allow_input_injection, "IPC socket ready");
        Ok(Self {
            path,
            connections: HashMap::new(),
            next_id: 1,
            listener_token: Some(token),
            allow_input_injection,
        })
    }

    /// Queue a response line for a connection and try to send it right away.
    fn send(&mut self, id: ConnectionId, response: &Response) -> Result<(), ()> {
        let Some(conn) = self.connections.get_mut(&id) else {
            return Ok(());
        };
        let Ok(mut line) = serde_json::to_vec(response) else {
            return Ok(());
        };
        line.push(b'\n');
        conn.write_buf.extend_from_slice(&line);
        if conn.write_buf.len() > MAX_PENDING_OUTPUT {
            tracing::warn!(id, "ipc client is not reading its responses; dropping it");
            return Err(());
        }
        flush(conn).map(|_| ())
    }

    pub fn broadcast(&mut self, event: Event) {
        let kind = event.kind();
        let ids: Vec<_> = self
            .connections
            .iter()
            .filter(|(_, c)| c.subscriptions.contains(&kind))
            .map(|(id, _)| *id)
            .collect();
        let response = Response::Event { event };
        for id in ids {
            let _ = self.send(id, &response);
        }
    }
}

/// Write as much buffered output as the socket takes. Ok(true) = everything sent.
fn flush(conn: &mut Connection) -> Result<bool, ()> {
    while !conn.write_buf.is_empty() {
        match conn.stream.write(&conn.write_buf) {
            Ok(0) => return Err(()),
            Ok(n) => {
                conn.write_buf.drain(..n);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return Err(()),
        }
    }
    Ok(true)
}

fn accept(state: &mut SpaceState, stream: UnixStream) {
    let Some(ipc) = state.ipc.as_mut() else {
        return;
    };
    let reject = |mut stream: UnixStream, msg: &str| {
        let _ = stream.set_nonblocking(true);
        if let Ok(mut line) = serde_json::to_vec(&Response::error(msg)) {
            line.push(b'\n');
            let _ = stream.write(&line);
        }
    };

    match rustix::net::sockopt::socket_peercred(&stream) {
        Ok(cred) if cred.uid == rustix::process::getuid() => {}
        Ok(cred) => {
            tracing::warn!(
                uid = cred.uid.as_raw(),
                pid = cred.pid.as_raw_nonzero().get(),
                "ipc: rejected other user"
            );
            return reject(stream, "permission denied");
        }
        Err(e) => {
            tracing::warn!("ipc: SO_PEERCRED failed: {e}");
            return reject(stream, "permission denied");
        }
    }
    if ipc.connections.len() >= MAX_CONNECTIONS {
        return reject(stream, "too many connections");
    }
    if stream.set_nonblocking(true).is_err() {
        return;
    }
    let Ok(reader) = stream.try_clone() else {
        return;
    };

    let id = ipc.next_id;
    ipc.next_id += 1;
    let token = state
        .loop_handle
        .insert_source(
            Generic::new(reader, Interest::READ, Mode::Level),
            move |_, _, state| Ok(on_readable(state, id)),
        )
        .ok();
    let Some(ipc) = state.ipc.as_mut() else {
        return;
    };
    ipc.connections.insert(
        id,
        Connection {
            stream,
            read_buf: Vec::new(),
            write_buf: Vec::new(),
            read_token: token,
            write_token: None,
            subscriptions: Vec::new(),
        },
    );
    tracing::debug!(id, "ipc client connected");
}

fn close(state: &mut SpaceState, id: ConnectionId) {
    let Some(ipc) = state.ipc.as_mut() else {
        return;
    };
    if let Some(conn) = ipc.connections.remove(&id) {
        for token in [conn.read_token, conn.write_token].into_iter().flatten() {
            state.loop_handle.remove(token);
        }
        tracing::debug!(id, "ipc client disconnected");
    }
}

fn on_readable(state: &mut SpaceState, id: ConnectionId) -> PostAction {
    let mut lines = Vec::new();
    let mut eof = false;
    let mut too_long = false;
    {
        let Some(conn) = state
            .ipc
            .as_mut()
            .and_then(|ipc| ipc.connections.get_mut(&id))
        else {
            return PostAction::Remove;
        };
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match conn.stream.read(&mut chunk) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => conn.read_buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    eof = true;
                    break;
                }
            }
            if conn.read_buf.len() > proto::MAX_REQUEST_BYTES * 4 {
                break;
            }
        }
        while let Some(pos) = conn.read_buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = conn.read_buf.drain(..=pos).collect();
            if line.len() > proto::MAX_REQUEST_BYTES + 1 {
                too_long = true;
                break;
            }
            lines.push(line);
        }
        if conn.read_buf.len() > proto::MAX_REQUEST_BYTES {
            too_long = true;
        }
    }

    for line in lines {
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(text) {
            Ok(request) => handle_request(state, id, request),
            Err(e) => Some(Response::error(format!("malformed request: {e}"))),
        };
        if let Some(response) = response {
            if reply(state, id, &response).is_err() {
                close(state, id);
                return PostAction::Remove;
            }
        }
    }

    if too_long {
        let _ = reply(
            state,
            id,
            &Response::error("request too large (max 64 KiB)"),
        );
        close(state, id);
        return PostAction::Remove;
    }
    if eof {
        close(state, id);
        return PostAction::Remove;
    }
    PostAction::Continue
}

/// Send a response; if the socket is full, wait for it to become writable.
fn reply(state: &mut SpaceState, id: ConnectionId, response: &Response) -> Result<(), ()> {
    let Some(ipc) = state.ipc.as_mut() else {
        return Ok(());
    };
    ipc.send(id, response)?;
    let Some(conn) = ipc.connections.get_mut(&id) else {
        return Ok(());
    };
    if !conn.write_buf.is_empty() && conn.write_token.is_none() {
        let Ok(writer) = conn.stream.try_clone() else {
            return Err(());
        };
        let token = state
            .loop_handle
            .insert_source(
                Generic::new(writer, Interest::WRITE, Mode::Level),
                move |_, _, state| {
                    let Some(conn) = state
                        .ipc
                        .as_mut()
                        .and_then(|ipc| ipc.connections.get_mut(&id))
                    else {
                        return Ok(PostAction::Remove);
                    };
                    match flush(conn) {
                        Ok(true) => {
                            conn.write_token = None;
                            Ok(PostAction::Remove)
                        }
                        Ok(false) => Ok(PostAction::Continue),
                        Err(()) => {
                            conn.write_token = None;
                            close(state, id);
                            Ok(PostAction::Remove)
                        }
                    }
                },
            )
            .ok();
        if let Some(conn) = state
            .ipc
            .as_mut()
            .and_then(|ipc| ipc.connections.get_mut(&id))
        {
            conn.write_token = token;
        }
    }
    Ok(())
}

fn finite(values: &[f64]) -> Result<(), Response> {
    if values.iter().all(|v| v.is_finite() && v.abs() < 1e7) {
        Ok(())
    } else {
        Err(Response::error("numbers must be finite and reasonable"))
    }
}

/// Returns `None` when the answer comes later (screenshots).
fn handle_request(state: &mut SpaceState, id: ConnectionId, request: Request) -> Option<Response> {
    tracing::debug!(id, ?request, "ipc request");
    Some(match request {
        Request::Version => Response::Version {
            version: env!("CARGO_PKG_VERSION").into(),
            backend: state.backend.name().into(),
        },
        Request::ListPanels => Response::Panels {
            panels: list_panels(state),
        },
        Request::FocusPanel { id: panel } => match state.window_by_id(panel) {
            Some(window) => {
                state.focus_window(Some(&window), smithay::utils::SERIAL_COUNTER.next_serial());
                state.queue_redraw();
                Response::Ok
            }
            None => Response::error(format!("no panel {panel}")),
        },
        Request::ClosePanel { id: panel } => match state.window_by_id(panel) {
            Some(window) => {
                if let Some(t) = window.toplevel() {
                    t.send_close();
                } else if let Some(x) = window.x11_surface() {
                    let _ = x.close();
                }
                Response::Ok
            }
            None => Response::error(format!("no panel {panel}")),
        },
        Request::Spawn { command } => match crate::spawn::spawn(&command, &state.socket_name) {
            Ok(pid) => Response::Spawned { pid },
            Err(e) => Response::error(e),
        },
        Request::Screenshot { path, force } => match validate_screenshot_path(&path, force) {
            Ok(path) => {
                state.pending_screenshots.push(ScreenshotRequest {
                    path,
                    force,
                    reply_to: Some(id),
                });
                state.queue_redraw();
                return None;
            }
            Err(e) => Response::error(e),
        },
        Request::Quit => {
            state.quit();
            Response::Ok
        }
        Request::Subscribe { events } => {
            if let Some(conn) = state
                .ipc
                .as_mut()
                .and_then(|ipc| ipc.connections.get_mut(&id))
            {
                conn.subscriptions = events;
            }
            Response::Ok
        }
        Request::Input { action } => {
            if !state.ipc.as_ref().is_some_and(|i| i.allow_input_injection) {
                return Some(Response::error(
                    "input injection is disabled (start spacewm with --allow-input-injection)",
                ));
            }
            match run_input(state, action) {
                Ok(()) => Response::Ok,
                Err(e) => e,
            }
        }
    })
}

fn press(p: PressMode) -> PressAction {
    match p {
        PressMode::Press => PressAction::Press,
        PressMode::Release => PressAction::Release,
        PressMode::Tap => PressAction::Tap,
    }
}

fn run_input(state: &mut SpaceState, action: InputAction) -> Result<(), Response> {
    match action {
        InputAction::Key { combo, state: p } => {
            inject::key_combo(state, &combo, press(p)).map_err(Response::error)
        }
        InputAction::Type { text } => {
            if text.chars().count() > 4096 {
                return Err(Response::error("text too long"));
            }
            inject::type_text(state, &text).map_err(Response::error)
        }
        InputAction::MoveTo { x, y } => {
            finite(&[x, y])?;
            inject::pointer_move_to(state, x, y).map_err(Response::error)
        }
        InputAction::MoveBy { dx, dy } => {
            finite(&[dx, dy])?;
            inject::pointer_move_by(state, dx, dy);
            Ok(())
        }
        InputAction::Button { button, state: p } => {
            let code = match button {
                MouseButton::Left => inject::BTN_LEFT,
                MouseButton::Right => inject::BTN_RIGHT,
                MouseButton::Middle => inject::BTN_MIDDLE,
            };
            inject::button(state, code, press(p));
            Ok(())
        }
        InputAction::Scroll { dx, dy } => {
            finite(&[dx, dy])?;
            inject::scroll(state, dx, dy);
            Ok(())
        }
    }
}

fn list_panels(state: &SpaceState) -> Vec<PanelInfo> {
    let focused = state.seat.get_keyboard().and_then(|k| k.current_focus());
    state
        .space
        .elements()
        .map(|window| {
            let surface = window.wl_surface();
            let (app_id, title) = match (window.x11_surface(), surface.as_deref()) {
                (Some(x11), _) => (Some(x11.class()), Some(x11.title())),
                (None, Some(s)) => (
                    crate::wayland::xdg_shell::toplevel_app_id(s),
                    crate::wayland::xdg_shell::toplevel_title(s),
                ),
                _ => (None, None),
            };
            let loc = state.space.element_location(window).unwrap_or_default();
            let size = window.geometry().size;
            PanelInfo {
                id: crate::state::window_id(window),
                app_id,
                title,
                position: [loc.x as f64, loc.y as f64, 0.0],
                size_px: [size.w, size.h],
                focused: matches!(&focused, Some(crate::focus::KeyboardFocusTarget::Window(w)) if w == window),
                kind: if window.x11_surface().is_some() { "x11" } else { "wayland" }.into(),
            }
        })
        .collect()
}

/// Screenshots may only be written inside $HOME, as a regular file (no symlink tricks),
/// and only overwrite an existing file when `force` is set.
fn validate_screenshot_path(path: &str, force: bool) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err("screenshot path must be absolute".into());
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let home = Path::new(&home)
        .canonicalize()
        .map_err(|e| format!("HOME: {e}"))?;
    let name = path.file_name().ok_or("screenshot path has no file name")?;
    let parent = path.parent().ok_or("screenshot path has no parent")?;
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("{}: {e}", parent.display()))?;
    if !parent.starts_with(&home) {
        return Err(format!("screenshot path must be inside {}", home.display()));
    }
    let full = parent.join(name);
    match std::fs::symlink_metadata(&full) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err("refusing to write through a symlink".into())
        }
        Ok(meta) if !meta.is_file() => Err("target exists and is not a regular file".into()),
        Ok(_) if !force => Err(format!(
            "{} exists (use force to overwrite)",
            full.display()
        )),
        _ => Ok(full),
    }
}

/// Called by backends right after rendering a frame that screenshots were waiting for.
pub fn finish_screenshots(
    pending: &mut Vec<ScreenshotRequest>,
    captured: Result<CapturedImage, String>,
    done: &Sender<ScreenshotDone>,
) {
    match captured {
        Ok(image) => {
            let mut requests = std::mem::take(pending);
            let last = requests.pop();
            for request in requests {
                let copy = CapturedImage {
                    width: image.width,
                    height: image.height,
                    rgba: image.rgba.clone(),
                };
                screenshot::save_async(request, copy, done.clone());
            }
            if let Some(request) = last {
                screenshot::save_async(request, image, done.clone());
            }
        }
        Err(e) => {
            tracing::warn!("screenshot capture failed: {e}");
            for request in pending.drain(..) {
                let _ = done.send(ScreenshotDone {
                    reply_to: request.reply_to,
                    result: Err(e.clone()),
                });
            }
        }
    }
}

/// A screenshot finished on its worker thread.
pub fn on_screenshot_done(state: &mut SpaceState, done: ScreenshotDone) {
    let response = match done.result {
        Ok((path, width, height)) => {
            tracing::info!(path = %path.display(), width, height, "screenshot saved");
            Response::Screenshot {
                path: path.display().to_string(),
                width,
                height,
            }
        }
        Err(e) => Response::error(e),
    };
    if let Some(id) = done.reply_to {
        if reply(state, id, &response).is_err() {
            close(state, id);
        }
    }
}

pub fn broadcast(state: &mut SpaceState, event: Event) {
    if let Some(ipc) = state.ipc.as_mut() {
        ipc.broadcast(event);
    }
}

impl IpcServer {
    pub fn shutdown(&mut self, loop_handle: &LoopHandle<'static, SpaceState>) {
        if let Some(token) = self.listener_token.take() {
            loop_handle.remove(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_paths_are_confined_to_home() {
        assert!(validate_screenshot_path("relative.png", false).is_err());
        assert!(validate_screenshot_path("/etc/evil.png", false).is_err());
        assert!(validate_screenshot_path("/tmp/x.png", false).is_err());
        if let Some(home) = std::env::var_os("HOME") {
            let ok = Path::new(&home).join("spacewm-path-test-that-does-not-exist.png");
            assert!(validate_screenshot_path(&ok.display().to_string(), false).is_ok());
            // `..` escaping home is resolved by canonicalize and rejected.
            let escape = Path::new(&home).join("../../etc/x.png");
            assert!(validate_screenshot_path(&escape.display().to_string(), false).is_err());
        }
    }
}
