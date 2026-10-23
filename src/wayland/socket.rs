//! The Wayland socket: how clients find and connect to us.
//!
//! Clients connect to the unix socket `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`. We listen on
//! it, hand every new connection to the Wayland `Display`, and let the `Display` read
//! client requests whenever its fd becomes readable.

use std::ffi::OsString;

use anyhow::Context;
use smithay::{
    reexports::{
        calloop::{generic::Generic, Interest, LoopHandle, Mode, PostAction},
        wayland_server::Display,
    },
    wayland::socket::ListeningSocketSource,
};

use crate::state::{new_client_state, SpaceState};

pub fn init_wayland_listener(
    display: Display<SpaceState>,
    loop_handle: &LoopHandle<'static, SpaceState>,
) -> anyhow::Result<OsString> {
    // Picks the first free name: wayland-1, wayland-2, ... (wayland-0 is often the host).
    let source = ListeningSocketSource::new_auto().context("no free wayland socket name")?;
    let socket_name = source.socket_name().to_os_string();

    loop_handle
        .insert_source(source, |stream, _, state| {
            match state
                .display_handle
                .insert_client(stream, new_client_state())
            {
                Ok(client) => tracing::info!(client = ?client.id(), "new client connected"),
                // A broken client must never take the compositor down.
                Err(err) => tracing::warn!("failed to insert new client: {err}"),
            }
        })
        .map_err(|e| anyhow::anyhow!("failed to insert wayland socket source: {}", e.error))?;

    // The Display's fd becomes readable when any client sent requests. Level mode: keep
    // waking us while there is unread data.
    loop_handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // SAFETY: we never drop the display while the source exists.
                let display = unsafe { display.get_mut() };
                if let Err(err) = display.dispatch_clients(state) {
                    tracing::error!("failed to dispatch clients: {err}");
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("failed to insert display source: {}", e.error))?;

    Ok(socket_name)
}
