//! ext-idle-notify and idle-inhibit: "has the user been away for N seconds?"
//!
//! swayidle asks to be told after, say, 300 s without input, and then runs swaylock or
//! turns screens off. Every key press, mouse move or click resets the clock
//! (`input_activity`). A video player can hold an *inhibitor* while its window shows a
//! video; while one exists, idle timers don't fire (except for clients that asked about
//! raw input idleness, which ignore inhibitors by design).

use smithay::{
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    wayland::{
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
    },
};

use crate::state::SpaceState;

impl IdleNotifierHandler for SpaceState {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle_notifier_state
    }
}

impl IdleInhibitHandler for SpaceState {
    fn inhibit(&mut self, surface: WlSurface) {
        tracing::info!(surface = surface.id().protocol_id(), "idle inhibited");
        self.idle_inhibitors.retain(|s| s.is_alive() && s != &surface);
        self.idle_inhibitors.push(surface);
        self.update_idle_inhibit();
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        tracing::info!(surface = surface.id().protocol_id(), "idle inhibit released");
        self.idle_inhibitors.retain(|s| s.is_alive() && s != &surface);
        self.update_idle_inhibit();
    }
}

impl SpaceState {
    /// Any user input: idle timers start over.
    pub fn input_activity(&mut self) {
        self.idle_notifier_state.notify_activity(&self.seat);
    }

    pub fn update_idle_inhibit(&mut self) {
        // An inhibitor of a client that died without releasing it doesn't count.
        self.idle_inhibitors.retain(|s| s.is_alive());
        let inhibited = !self.idle_inhibitors.is_empty();
        self.idle_notifier_state.set_is_inhibited(inhibited);
    }
}
