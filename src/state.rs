//! `SpaceState`: the single struct that owns all compositor state.
//!
//! It will grow to hold the Wayland Display handle, every protocol's state, the
//! world, the camera and the input mode. smithay's handler traits
//! (CompositorHandler, XdgShellHandler, SeatHandler, ...) are implemented on
//! this one type, and calloop passes `&mut SpaceState` to every event callback.
//! That is why everything lives in ONE struct: any callback can reach any state
//! without Rc<RefCell<..>> or locks.

use std::time::Instant;

use crate::cli::Cli;

pub struct SpaceState {
    pub running: bool,
    pub start_time: Instant,
    pub cli: Cli,
}

impl SpaceState {
    pub fn new(cli: Cli) -> Self {
        Self {
            running: true,
            start_time: Instant::now(),
            cli,
        }
    }
}
