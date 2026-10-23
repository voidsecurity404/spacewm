//! Backends: where frames are shown and where input comes from.
//!
//! winit (Stage 1) runs spacewm as a window inside the current desktop; drm
//! (Stage 6) drives the GPU and input devices directly from a TTY.
//!
//! A trait object keeps main.rs simple for now. smithay's backend types are
//! heavily generic, so we may replace this trait with an enum later if trait
//! objects become awkward.

mod drm;
mod winit;

pub use crate::cli::BackendKind;
use crate::{error::SpaceError, state::SpaceState};

pub trait Backend {
    fn name(&self) -> &'static str;
    fn init(&mut self, state: &mut SpaceState) -> Result<(), SpaceError>;
}

pub fn create(kind: BackendKind) -> Box<dyn Backend> {
    match kind {
        BackendKind::Winit => Box::new(winit::WinitBackend),
        BackendKind::Drm => Box::new(drm::DrmBackend),
    }
}
