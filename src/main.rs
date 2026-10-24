//! spacewm entry point: parse CLI, init logging, start the backend and Wayland socket,
//! then run the event loop. All real work lives in the modules.

mod backend;
mod cli;
mod config;
mod error;
mod focus;
mod input;
mod ipc;
mod launcher;
mod logging;
mod render;
mod spawn;
mod state;
mod wayland;
mod world;
mod xwayland;

use anyhow::{anyhow, Context};
use clap::Parser;
// calloop is the event loop smithay is built on. A compositor is a reactor: it sleeps
// until something happens (client request, input event, vblank, timer, signal) and
// then runs the matching callback with `&mut SpaceState`. No threads, no locks.
use smithay::{
    backend::renderer::ImportMemWl,
    reexports::{
        calloop::{
            signals::{Signal, Signals},
            timer::{TimeoutAction, Timer},
            EventLoop,
        },
        wayland_server::Display,
    },
};

use crate::{backend::Backend, cli::Cli, state::SpaceState};

fn main() -> anyhow::Result<()> {
    block_shutdown_signals();
    let mut cli = Cli::parse();
    if cli.check_config {
        return check_config(cli.config.as_deref());
    }
    if cli.drm_probe {
        // Read-only listing; needs no session, logging setup or event loop.
        return backend::drm::probe();
    }
    if rustix::process::geteuid().is_root() {
        // Root is never needed: libseat hands the GPU and input devices to a normal user.
        // A root compositor would make every bug (and every client) far more dangerous.
        return Err(anyhow!(
            "do not run spacewm as root (no sudo): start it as your user from a TTY login"
        ));
    }
    if cli.drm_test {
        // A smoke test of GPU + monitor: the bare world, no clients, quit after 10 s.
        cli.backend = cli::BackendKind::Drm;
        cli.spawn.clear();
    }
    let log_path = logging::init(&cli.log_level).context("failed to init logging")?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        backend = ?cli.backend,
        render = ?cli.render,
        log = %log_path.display(),
        "starting spacewm"
    );
    // Errors also go to the log file: on a TTY nobody may be watching stderr.
    run(cli).inspect_err(|err| tracing::error!("fatal: {err:#}"))
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let config_path = match &cli.config {
        Some(path) => path.clone(),
        None => config::default_path()?,
    };
    // A broken config doesn't stop spacewm: it starts with defaults and shows the error.
    let (config, config_error) = config::load_or_create(&config_path);
    match &config_error {
        Some(e) => tracing::error!("config not loaded, using defaults:\n{e}"),
        None => tracing::info!(path = %config_path.display(), "config loaded"),
    }

    let mut event_loop: EventLoop<SpaceState> =
        EventLoop::try_new().context("failed to create event loop")?;
    let loop_handle = event_loop.handle();
    let mut display: Display<SpaceState> = Display::new().context("failed to create display")?;

    let backend = Backend::init(cli.backend, &cli, &config, &display.handle(), &loop_handle)
        .with_context(|| format!("failed to start {:?} backend", cli.backend))?;
    tracing::info!(backend = backend.name(), "backend ready");

    let mut state = SpaceState::new(
        &mut display,
        loop_handle.clone(),
        event_loop.get_signal(),
        cli,
        backend,
        config.clone(),
        config_path.clone(),
    )?;
    state.apply_config(config, true);
    match config::watch::watch(&config_path, &loop_handle) {
        Ok(watcher) => state.config_watcher = Some(watcher),
        Err(err) => tracing::warn!("config changes won't be picked up live: {err:#}"),
    }

    // Tell clients which shm pixel formats our renderer can import.
    let shm_formats = state.backend.renderer().shm_formats();
    state.shm_state.update_formats(shm_formats);

    // Lay outputs out side by side in the 2D space.
    let mut x = 0;
    for output in state.backend.outputs() {
        let width = output.current_mode().map(|m| m.size.w).unwrap_or(0);
        state.space.map_output(&output, (x, 0));
        x += width;
    }
    if let Some(geo) = state.primary_output_geometry() {
        state.pointer_location = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
    }

    let socket_name = wayland::socket::init_wayland_listener(display, &loop_handle)?;
    // Our own WAYLAND_DISPLAY is NOT changed: setenv while other threads (Mesa's) may
    // call getenv is a data race. Children get the variable explicitly (spawn.rs).
    tracing::info!(socket = ?socket_name, "spacewm listening on {}", socket_name.to_string_lossy());
    state.socket_name = socket_name.clone();

    if !state.cli.no_xwayland {
        match xwayland::Xwayland::init(&loop_handle) {
            Ok(xw) => {
                spawn::set_x_display(Some(xw.display));
                state.xwayland = Some(xw);
            }
            Err(err) => tracing::warn!("X11 apps disabled: {err:#}"),
        }
    }

    let ipc_path = state.cli.ipc_socket.clone().or_else(|| ipc::default_socket_path(&socket_name));
    match ipc_path {
        Some(path) => {
            match ipc::IpcServer::start(path, state.cli.allow_input_injection, &loop_handle) {
                Ok(server) => {
                    spawn::set_ipc_socket(Some(server.path.clone()));
                    state.ipc = Some(server);
                }
                Err(err) => tracing::warn!("IPC disabled: {err:#}"),
            }
        }
        None => tracing::warn!("IPC disabled: XDG_RUNTIME_DIR is not set"),
    }

    // Ctrl+C / kill: stop the loop instead of dying mid-frame, so we clean up.
    let signals =
        Signals::new(&[Signal::SIGINT, Signal::SIGTERM]).context("failed to set up signals")?;
    loop_handle
        .insert_source(signals, |event, _, state: &mut SpaceState| {
            tracing::info!("received {:?}, shutting down", event.signal());
            state.quit();
        })
        .map_err(|e| anyhow!("failed to register signal source: {}", e.error))?;

    // Frame callbacks normally go out after each rendered frame. A scene where nothing
    // moves renders nothing, so apps on panels out of view would wait forever (a terminal
    // asked to resize never draws its new size). This tick hands them their throttled
    // once-a-second callback without rendering.
    let tick = std::time::Duration::from_secs(1);
    loop_handle
        .insert_source(Timer::from_duration(tick), move |_, _, state: &mut SpaceState| {
            state.send_hidden_frame_callbacks();
            TimeoutAction::ToDuration(tick)
        })
        .map_err(|e| anyhow!("failed to register frame callback timer: {}", e.error))?;

    if state.cli.drm_test {
        let after = std::time::Duration::from_secs(10);
        loop_handle
            .insert_source(Timer::from_duration(after), |_, _, state: &mut SpaceState| {
                tracing::info!("--drm-test: 10 s passed, exiting");
                state.quit();
                TimeoutAction::Drop
            })
            .map_err(|e| anyhow!("failed to register test timer: {}", e.error))?;
    }

    if state.cli.backend == cli::BackendKind::Drm && !state.cli.no_session_env {
        // On a real session we are the first program: tell D-Bus activated services
        // (portals, notification daemons) and systemd user units where our Wayland socket
        // is, or they would start without WAYLAND_DISPLAY and fail.
        spawn::update_activation_environment(&state.socket_name);
    }

    if let Some(e) = config_error {
        state.show_error(e.short());
    }

    let autostart = state.config.general.autostart.clone();
    for command in state.cli.spawn.clone().into_iter().chain(autostart) {
        if let Err(err) = spawn::spawn(&command, &state.socket_name) {
            tracing::warn!("{err}");
        }
    }

    state.queue_redraw();
    while state.running {
        event_loop.dispatch(None, &mut state).context("event loop dispatch failed")?;
        state.refresh_and_flush();
    }

    state.save_layout();
    // GL objects can only be freed while the context exists, i.e. before the backend drops.
    if let Some(scene) = state.view.scene.take() {
        scene.destroy(state.backend.renderer());
    }
    if let Some(mut ipc) = state.ipc.take() {
        ipc.shutdown(&loop_handle);
    }
    if let Some(mut xw) = state.xwayland.take() {
        xw.shutdown(&loop_handle);
    }
    tracing::info!(uptime_s = state.start_time.elapsed().as_secs_f32(), "spacewm exited cleanly");
    Ok(())
}

/// Block SIGINT/SIGTERM in this thread before any other thread exists.
///
/// We receive these signals through a signalfd (calloop `Signals`), which only works if
/// the signal is blocked in EVERY thread: otherwise the kernel delivers a process-wide
/// signal to some thread that doesn't block it (our mesher, a Mesa worker) and its
/// default action kills spacewm on the spot — no layout save, no clean DRM teardown. New
/// threads inherit the creating thread's mask, so blocking here, first, covers them all.
/// (Children get a clean mask again in spawn.rs.)
fn block_shutdown_signals() {
    // SAFETY: plain libc signal-set manipulation on a local set; pthread_sigmask only
    // changes this thread's mask.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// `spacewm --check-config`: validate and report, exit code 0 or 1.
fn check_config(path: Option<&std::path::Path>) -> anyhow::Result<()> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => config::default_path()?,
    };
    match config::load(&path) {
        Ok(_) => {
            println!("{}: OK", path.display());
            Ok(())
        }
        Err(e) => {
            let text = e.to_string();
            eprint!("{text}");
            if !text.ends_with('\n') {
                eprintln!();
            }
            std::process::exit(1);
        }
    }
}
