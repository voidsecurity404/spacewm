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

        // Children find us through SPACEWM_SOCKET, which spawn.rs sets for them.
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
        .insert_source(Generic::new(reader, Interest::READ, Mode::Level), move |_, _, state| {
            Ok(on_readable(state, id))
        })
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
    let mut eof = false;
    let (lines, too_long) = {
        let Some(conn) = state.ipc.as_mut().and_then(|ipc| ipc.connections.get_mut(&id)) else {
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
        take_lines(&mut conn.read_buf)
    };

    for line in lines {
        let response = match parse_request(&line) {
            None => continue,
            Some(Ok(request)) => handle_request(state, id, request),
            Some(Err(e)) => Some(e),
        };
        if let Some(response) = response {
            if reply(state, id, &response).is_err() {
                close(state, id);
                return PostAction::Remove;
            }
        }
    }

    if too_long {
        let _ = reply(state, id, &Response::error("request too large (max 64 KiB)"));
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
                    let Some(conn) =
                        state.ipc.as_mut().and_then(|ipc| ipc.connections.get_mut(&id))
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
        if let Some(conn) = state.ipc.as_mut().and_then(|ipc| ipc.connections.get_mut(&id)) {
            conn.write_token = token;
        }
    }
    Ok(())
}

/// Split complete request lines off the front of `buf`. `true` if a line (complete or
/// still growing) is over the size limit: the client is then dropped.
fn take_lines(buf: &mut Vec<u8>) -> (Vec<Vec<u8>>, bool) {
    let mut lines = Vec::new();
    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = buf.drain(..=pos).collect();
        if line.len() > proto::MAX_REQUEST_BYTES + 1 {
            return (lines, true);
        }
        lines.push(line);
    }
    (lines, buf.len() > proto::MAX_REQUEST_BYTES)
}

/// Parse one request line; anything that isn't a valid request gets an error response.
fn parse_request(line: &[u8]) -> Option<Result<Request, Response>> {
    let text = std::str::from_utf8(line).map_err(|_| Response::error("request is not UTF-8"));
    let text = match text {
        Ok(t) => t.trim(),
        Err(e) => return Some(Err(e)),
    };
    if text.is_empty() {
        return None;
    }
    Some(serde_json::from_str(text).map_err(|e| Response::error(format!("malformed request: {e}"))))
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
        Request::ListPanels => Response::Panels { panels: list_panels(state) },
        Request::FocusPanel { id: panel } => match state.window_by_id(panel) {
            Some(_) if state.render_mode == crate::cli::RenderMode::Scene3D => {
                crate::input::mode::set_mode(
                    state,
                    crate::input::mode::InputMode::Window { panel },
                    None,
                );
                Response::Ok
            }
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
        Request::Spawn { command, shell } => {
            if shell && !state.config.general.ipc_allow_shell {
                return Some(Response::error(
                    "shell commands are disabled (set general.ipc_allow_shell = true in the config)",
                ));
            }
            // The window this opens may take focus (see `mode::on_new_window`).
            state.expect_window_until =
                Some(std::time::Instant::now() + crate::input::mode::SPAWN_FOCUS_WINDOW);
            let result = if shell {
                crate::spawn::spawn_shell(&command, &state.socket_name)
            } else {
                // Scripts and tests: no "Starting ..." line.
                state.launch_untracked(&command)
            };
            match result {
                Ok(pid) => Response::Spawned { pid },
                Err(e) => Response::error(e),
            }
        }
        Request::MovePanel { id: panel, position, yaw_deg } => {
            let mut values: Vec<f64> = position.iter().map(|&v| v as f64).collect();
            values.extend(yaw_deg.map(|v| v as f64));
            if let Err(e) = finite(&values) {
                return Some(e);
            }
            if position.iter().any(|v| v.abs() > 10_000.0) {
                return Some(Response::error("position out of range (±10000 m)"));
            }
            let Some(p) = state.world.panels_mut().iter_mut().find(|p| p.id == panel) else {
                return Some(Response::error(format!("no panel {panel}")));
            };
            p.position = glam::Vec3::from(position);
            if let Some(yaw) = yaw_deg {
                p.rotation = glam::Quat::from_rotation_y(yaw.to_radians());
            }
            if state.grab.is_some_and(|g| g.panel == panel) {
                state.grab = None; // the explicit position wins over carrying it
            }
            panel_changed(state)
        }
        Request::ResizePanel { id: panel, width, height } => {
            if !(100..=8000).contains(&width) || !(100..=8000).contains(&height) {
                return Some(Response::error("width and height must be 100..8000"));
            }
            match state.window_by_id(panel) {
                Some(window) => {
                    crate::input::world_ops::resize_window(&window, (width, height));
                    state.queue_redraw();
                    Response::Ok
                }
                None => Response::error(format!("no panel {panel}")),
            }
        }
        Request::ScalePanel { id: panel, pixels_per_unit } => {
            if !(200.0..=3000.0).contains(&pixels_per_unit) {
                return Some(Response::error("pixels_per_unit must be 200..3000"));
            }
            let Some(p) = state.world.panels_mut().iter_mut().find(|p| p.id == panel) else {
                return Some(Response::error(format!("no panel {panel}")));
            };
            p.pixels_per_unit = pixels_per_unit;
            panel_changed(state)
        }
        Request::SetMode { mode } => {
            if state.render_mode != crate::cli::RenderMode::Scene3D {
                return Some(Response::error("modes exist only in the 3D view"));
            }
            let target = match mode {
                proto::ModeTarget::Walk => crate::input::mode::InputMode::Walk,
                proto::ModeTarget::Window(panel) => {
                    if !state.world.panels().iter().any(|p| p.id == panel) {
                        return Some(Response::error(format!("no panel {panel}")));
                    }
                    crate::input::mode::InputMode::Window { panel }
                }
            };
            crate::input::mode::set_mode(state, target, None);
            Response::Ok
        }
        Request::Camera => {
            let c = &state.view.camera;
            Response::Camera {
                position: c.position.to_array().map(f64::from),
                yaw_deg: f64::from(c.yaw.to_degrees()),
                pitch_deg: f64::from(c.pitch.to_degrees()),
            }
        }
        Request::Stats => {
            let s = state.perf.snapshot(std::time::Instant::now());
            let gpu = state.view.scene.as_ref().map_or(0, |sc| sc.gpu_bytes())
                + state.view.panel_textures.as_ref().map_or(0, |t| t.gpu_bytes());
            Response::Stats {
                stats: proto::Stats {
                    fps: s.fps,
                    frame_ms_p50: s.frame_ms_p50,
                    frame_ms_p95: s.frame_ms_p95,
                    frame_ms_p99: s.frame_ms_p99,
                    frame_ms_max: s.frame_ms_max,
                    panel_updates_per_s: s.panel_updates_per_s,
                    chunk_uploads_per_s: s.chunk_uploads_per_s,
                    gpu_memory_mib: gpu as f64 / (1024.0 * 1024.0),
                    panels: state.world.panels().len(),
                    visible_panels: state.view.visible_panels.len(),
                },
            }
        }
        Request::ReloadConfig => match state.reload_config() {
            Ok(()) => Response::Ok,
            Err(e) => Response::error(e),
        },
        // Locked screens are not captured. Test mode (--allow-input-injection, which can
        // type anything anyway) may, so tests can check what the lock screen shows.
        Request::Screenshot { .. }
            if state.lock.is_locked()
                && !state.ipc.as_ref().is_some_and(|i| i.allow_input_injection) =>
        {
            Response::error("the screen is locked")
        }
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
        Request::Teleport { position, yaw_deg, pitch_deg } => {
            let mut values: Vec<f64> = position.iter().map(|&v| v as f64).collect();
            values.extend(yaw_deg.map(|v| v as f64));
            values.extend(pitch_deg.map(|v| v as f64));
            if let Err(e) = finite(&values) {
                return Some(e);
            }
            // A glide in progress would move the view away again.
            state.focus_view_cancel();
            let camera = &mut state.view.camera;
            camera.position = glam::Vec3::from(position);
            // Teleporting puts you in fly mode, so you stay where you were put instead of
            // falling (double-tap Space to walk again).
            state.player = crate::world::player::Player::new(
                camera.position - glam::Vec3::new(0.0, crate::world::player::EYE_HEIGHT, 0.0),
            );
            state.player.flying = true;
            if let Some(yaw) = yaw_deg {
                camera.yaw = yaw.to_radians().rem_euclid(std::f32::consts::TAU);
            }
            if let Some(pitch) = pitch_deg {
                camera.pitch = pitch.clamp(-89.0, 89.0).to_radians();
            }
            state.view.dirty = true;
            state.queue_redraw();
            Response::Ok
        }
        Request::SetBlock { pos, block } => {
            if pos.iter().any(|v| v.abs() > 100_000) {
                return Some(Response::error("position out of range"));
            }
            match serde_json::from_value::<crate::world::block::Block>(serde_json::Value::String(
                block.clone(),
            )) {
                Ok(b) => {
                    let p = glam::IVec3::from(pos);
                    state.voxels.set_block(p, b);
                    state.layout.record_block(p, b);
                    state.layout_changed();
                    state.queue_redraw();
                    Response::Ok
                }
                Err(_) => Response::error(format!("unknown block '{block}'")),
            }
        }
        Request::GetBlock { pos } => {
            let b = state.voxels.map.block_at(glam::IVec3::from(pos));
            Response::Block {
                block: serde_json::to_value(b)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default(),
            }
        }
        Request::Subscribe { events } => {
            if let Some(conn) = state.ipc.as_mut().and_then(|ipc| ipc.connections.get_mut(&id)) {
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

/// A panel moved or was rescaled by IPC: redraw and remember it like a manual change.
fn panel_changed(state: &mut SpaceState) -> Response {
    state.view.dirty = true;
    state.layout_changed();
    state.queue_redraw();
    Response::Ok
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
        // X11 menus/tooltips live inside their parent's panel, they are not windows.
        .filter(|w| {
            state.world.panel_for_window(w).is_some()
                || !w.x11_surface().is_some_and(|x| x.is_override_redirect())
        })
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
            let size = window.geometry().size;
            // 3D position of the panel (metres); 2D-only windows fall back to their place
            // in the 2D space.
            let (position, yaw_deg, pixels_per_unit, visible) = match state.world.panel_for_window(window) {
                Some(p) => {
                    let n = p.normal();
                    (
                        [p.position.x as f64, p.position.y as f64, p.position.z as f64],
                        (n.x as f64).atan2(n.z as f64).to_degrees(),
                        p.pixels_per_unit as f64,
                        p.visible,
                    )
                }
                None => {
                    let loc = state.space.element_location(window).unwrap_or_default();
                    ([loc.x as f64, loc.y as f64, 0.0], 0.0, 0.0, true)
                }
            };
            PanelInfo {
                id: crate::state::window_id(window),
                app_id,
                title,
                position,
                yaw_deg,
                pixels_per_unit,
                visible,
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
    let home = Path::new(&home).canonicalize().map_err(|e| format!("HOME: {e}"))?;
    let name = path.file_name().ok_or("screenshot path has no file name")?;
    let parent = path.parent().ok_or("screenshot path has no parent")?;
    let parent = parent.canonicalize().map_err(|e| format!("{}: {e}", parent.display()))?;
    if !parent.starts_with(&home) {
        return Err(format!("screenshot path must be inside {}", home.display()));
    }
    let full = parent.join(name);
    match std::fs::symlink_metadata(&full) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err("refusing to write through a symlink".into())
        }
        Ok(meta) if !meta.is_file() => Err("target exists and is not a regular file".into()),
        Ok(_) if !force => Err(format!("{} exists (use force to overwrite)", full.display())),
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
                let _ = done
                    .send(ScreenshotDone { reply_to: request.reply_to, result: Err(e.clone()) });
            }
        }
    }
}

/// A screenshot finished on its worker thread.
pub fn on_screenshot_done(state: &mut SpaceState, done: ScreenshotDone) {
    let response = match done.result {
        Ok((path, width, height)) => {
            tracing::info!(path = %path.display(), width, height, "screenshot saved");
            Response::Screenshot { path: path.display().to_string(), width, height }
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
    fn requests_are_split_into_lines() {
        let mut buf = b"{\"request\":\"version\"}\n{\"request\":\"quit\"}\n{\"requ".to_vec();
        let (lines, too_long) = take_lines(&mut buf);
        assert_eq!(lines.len(), 2);
        assert!(!too_long);
        assert_eq!(buf, b"{\"requ", "the partial line waits for more data");
        assert_eq!(parse_request(&lines[0]), Some(Ok(Request::Version)));
        assert_eq!(parse_request(&lines[1]), Some(Ok(Request::Quit)));
    }

    #[test]
    fn oversized_requests_are_rejected() {
        // A complete line over the limit...
        let mut buf = vec![b'x'; proto::MAX_REQUEST_BYTES + 10];
        buf.push(b'\n');
        assert!(take_lines(&mut buf).1);
        // ...and one still growing without a newline.
        let mut buf = vec![b'x'; proto::MAX_REQUEST_BYTES + 1];
        assert!(take_lines(&mut buf).1);
        // Exactly at the limit is fine.
        let mut buf = vec![b' '; proto::MAX_REQUEST_BYTES];
        buf.push(b'\n');
        assert!(!take_lines(&mut buf).1);
    }

    #[test]
    fn malformed_requests_get_errors_not_panics() {
        for bad in [
            &b"not json"[..],
            b"{\"request\":\"spawn\"}",
            b"{\"request\":\"move_panel\",\"id\":1,\"position\":[1,2]}",
            b"{\"request\":\"focus_panel\",\"id\":-1}",
            b"\xff\xfe",
            b"[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[",
        ] {
            assert!(matches!(parse_request(bad), Some(Err(Response::Error { .. }))), "{bad:?}");
        }
        assert_eq!(parse_request(b"   \r"), None, "blank lines are ignored");
    }

    #[test]
    fn numbers_must_be_finite() {
        assert!(finite(&[1.0, -2.5]).is_ok());
        assert!(finite(&[f64::NAN]).is_err());
        assert!(finite(&[f64::INFINITY]).is_err());
        assert!(finite(&[1e9]).is_err());
        // JSON has no NaN literal, so it can't even be sent.
        assert!(parse_request(b"{\"request\":\"teleport\",\"position\":[NaN,0,0]}")
            .is_some_and(|r| r.is_err()));
    }

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
