//! spacewm entry point: parse CLI, init logging, build state, pick a backend and
//! run the event loop. All real work lives in the modules.

mod backend;
mod cli;
mod config;
mod error;
mod input;
mod logging;
mod render;
mod state;
mod wayland;
mod world;

use std::time::Duration;

use anyhow::{anyhow, Context};
use clap::Parser;
// calloop is the event loop smithay is built on. A compositor is a reactor:
// it sleeps until something happens (client message, input event, vblank,
// timer, signal) and then runs the matching callback with `&mut SpaceState`.
use smithay::reexports::calloop::{
    signals::{Signal, Signals},
    timer::{TimeoutAction, Timer},
    EventLoop,
};

use crate::{cli::Cli, state::SpaceState};

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
    let handle = event_loop.handle();

    let mut state = SpaceState::new(cli);

    let mut backend = backend::create(state.cli.backend);
    backend
        .init(&mut state)
        .with_context(|| format!("failed to start {} backend", backend.name()))?;

    // Ctrl+C / kill: stop the loop instead of dying mid-frame, so we can clean up.
    let signals =
        Signals::new(&[Signal::SIGINT, Signal::SIGTERM]).context("failed to set up signals")?;
    handle
        .insert_source(signals, |event, _, state: &mut SpaceState| {
            tracing::info!("received {:?}, shutting down", event.signal());
            state.running = false;
        })
        .map_err(|e| anyhow!("failed to register signal source: {}", e.error))?;

    // TEMPORARY (Stage 0 only): stop after 2 s so `cargo run` exits by itself.
    handle
        .insert_source(
            Timer::from_duration(Duration::from_secs(2)),
            |_, _, state| {
                tracing::info!("stage 0 timer elapsed, stopping");
                state.running = false;
                TimeoutAction::Drop
            },
        )
        .map_err(|e| anyhow!("failed to register timer: {}", e.error))?;

    while state.running {
        // ~60 Hz upper bound on how long we sleep with nothing to do.
        event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .context("event loop dispatch failed")?;
    }

    tracing::info!(
        uptime_s = state.start_time.elapsed().as_secs_f32(),
        "spacewm exited cleanly"
    );
    Ok(())
}
