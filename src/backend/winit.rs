//! winit backend: spacewm as a window inside the current desktop.
//!
//! Stage 1 fills this in: a GLES renderer on a winit window, one Output the
//! size of that window, and host keyboard/mouse events forwarded to our input
//! code. It is the safe way to develop — a hang only freezes one window.

use super::Backend;
use crate::{error::SpaceError, state::SpaceState};

pub struct WinitBackend;

impl Backend for WinitBackend {
    fn name(&self) -> &'static str {
        "winit"
    }

    fn init(&mut self, _state: &mut SpaceState) -> Result<(), SpaceError> {
        tracing::info!("{} backend: not implemented yet (stage 1)", self.name());
        Ok(())
    }
}
