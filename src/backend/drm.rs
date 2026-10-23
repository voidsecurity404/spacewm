//! DRM/KMS backend: spacewm as a real session on a TTY.
//!
//! Stage 6 fills this in: libseat for device access without root, udev for
//! GPUs and monitors, GBM + EGL for rendering, vblank-driven page flips, and
//! libinput for keyboards, mice and touchpads.

use smithay::{
    backend::renderer::glow::GlowRenderer,
    output::Output,
    reexports::{calloop::LoopHandle, wayland_server::DisplayHandle},
    wayland::dmabuf::DmabufState,
};

use crate::{error::SpaceError, state::SpaceState};

pub struct DrmBackend {
    // Uninhabited until Stage 6: `init` always fails, so no value of this type exists.
    never: std::convert::Infallible,
}

impl DrmBackend {
    pub fn init(
        _dh: &DisplayHandle,
        _loop_handle: &LoopHandle<'static, SpaceState>,
    ) -> Result<Self, SpaceError> {
        Err(SpaceError::Backend("drm backend: not implemented yet (stage 6)".into()))
    }

    pub fn renderer(&mut self) -> &mut GlowRenderer {
        match self.never {}
    }

    pub fn dmabuf_state(&mut self) -> &mut DmabufState {
        match self.never {}
    }

    pub fn outputs(&self) -> Vec<Output> {
        match self.never {}
    }

    pub fn schedule_redraw(&mut self) {
        match self.never {}
    }
}
