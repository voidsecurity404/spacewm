//! DRM/KMS backend: spacewm as a real session on a TTY.
//!
//! Stage 6 fills this in: libseat for device access without root, udev for
//! GPUs and monitors, GBM + EGL for rendering, vblank-driven page flips, and
//! libinput for keyboards, mice and touchpads.

use super::Backend;
use crate::{error::SpaceError, state::SpaceState};

pub struct DrmBackend;

impl Backend for DrmBackend {
    fn name(&self) -> &'static str {
        "drm"
    }

    fn init(&mut self, _state: &mut SpaceState) -> Result<(), SpaceError> {
        tracing::info!("{} backend: not implemented yet (stage 6)", self.name());
        Ok(())
    }
}
