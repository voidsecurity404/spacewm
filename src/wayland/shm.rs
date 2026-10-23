//! wl_shm: shared-memory buffers.
//!
//! The client draws on the CPU into memory it shares with us through a file descriptor
//! (memfd). Simple and universal; terminals like foot use it. GPU clients use DMA-BUF
//! instead (see dmabuf.rs), which avoids copying pixels through the CPU.

use smithay::wayland::shm::{ShmHandler, ShmState};

use crate::state::SpaceState;

impl ShmHandler for SpaceState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
