//! xdg-activation: an app asks for one of its windows to be brought forward ("open this
//! link" makes Firefox ask for its window, a finished download asks for attention).
//!
//! spacewm never moves keyboard focus for this — a window that grabs your typing while you
//! are in another one is a classic way to leak a password into the wrong app. Instead
//! the panel's border pulses for a few seconds so you can see where it is and walk over.
//! Tokens older than 10 s are ignored (they come from long-gone user actions).

use std::time::{Duration, Instant};

use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::xdg_activation::{
        XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
    },
};

use crate::state::SpaceState;

const MAX_TOKEN_AGE: Duration = Duration::from_secs(10);
/// How long a panel pulses.
pub const ATTENTION: Duration = Duration::from_secs(3);

impl XdgActivationHandler for SpaceState {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // One use per token; stale ones too.
        self.activation_state.remove_token(&token);
        if token_data.timestamp.elapsed() > MAX_TOKEN_AGE {
            tracing::debug!("activation request with a stale token ignored");
            return;
        }
        let Some(window) = self.window_for_surface(&surface) else { return };
        let id = crate::state::window_id(&window);
        tracing::info!(id, "panel asks for attention");
        if let Some(p) = self.world.panels_mut().iter_mut().find(|p| p.window == window) {
            p.attention_until = Some(Instant::now() + ATTENTION);
        }
        if self.render_mode == crate::cli::RenderMode::Flat2D {
            // 2D: bring it to the front, still without taking the keyboard.
            self.space.raise_element(&window, false);
        }
        // One more frame once the pulse is over, to take the border away.
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        let _ = self.loop_handle.insert_source(
            Timer::from_duration(ATTENTION + Duration::from_millis(30)),
            |_, _, state| {
                state.view.dirty = true;
                state.queue_redraw();
                TimeoutAction::Drop
            },
        );
        self.view.dirty = true;
        self.queue_redraw();
    }
}
