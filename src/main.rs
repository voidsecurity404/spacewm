//! spacewm entry point: parse CLI, init logging, start the backend and Wayland socket,
//! then run the event loop. All real work lives in the modules.

mod backend;
mod cli;
mod config;
mod error;
mod focus;
mod input;
mod ipc;
mod logging;
mod render;
mod spawn;
mod state;
mod wayland;
mod world;

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
            EventLoop,
        },
        wayland_server::Display,
    },
};

use crate::{backend::Backend, cli::Cli, state::SpaceState};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let log_path = logging::init(&cli.log_level).context("failed to init logging")?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        backend = ?cli.backend,
        render = ?cli.render,
        log = %log_path.display(),
        "starting spacewm"
    );

    let mut event_loop: EventLoop<SpaceState> =
        EventLoop::try_new().context("failed to create event loop")?;
    let loop_handle = event_loop.handle();
    let mut display: Display<SpaceState> = Display::new().context("failed to create display")?;

    let backend = Backend::init(cli.backend, &cli, &display.handle(), &loop_handle)
        .with_context(|| format!("failed to start {:?} backend", cli.backend))?;
    tracing::info!(backend = backend.name(), "backend ready");

    let mut state = SpaceState::new(
        &mut display,
        loop_handle.clone(),
        event_loop.get_signal(),
        cli,
        backend,
    )?;

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
    // SAFETY: still single-threaded; children inherit this variable.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &socket_name) };
    tracing::info!(socket = ?socket_name, "spacewm listening on {}", socket_name.to_string_lossy());
    state.socket_name = socket_name.clone();

    let ipc_path = state
        .cli
        .ipc_socket
        .clone()
        .or_else(|| ipc::default_socket_path(&socket_name));
    match ipc_path {
        Some(path) => {
            match ipc::IpcServer::start(path, state.cli.allow_input_injection, &loop_handle) {
                Ok(server) => state.ipc = Some(server),
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

    for command in state.cli.spawn.clone() {
        if let Err(err) = spawn::spawn(&command, &state.socket_name) {
            tracing::warn!("{err}");
        }
    }

    state.queue_redraw();
    while state.running {
        event_loop
            .dispatch(None, &mut state)
            .context("event loop dispatch failed")?;
        state.refresh_and_flush();
    }

    // GL objects can only be freed while the context exists, i.e. before the backend drops.
    if let Some(scene) = state.view.scene.take() {
        scene.destroy(state.backend.renderer());
    }
    if let Some(mut ipc) = state.ipc.take() {
        ipc.shutdown(&loop_handle);
    }
    tracing::info!(
        uptime_s = state.start_time.elapsed().as_secs_f32(),
        "spacewm exited cleanly"
    );
    Ok(())
}
