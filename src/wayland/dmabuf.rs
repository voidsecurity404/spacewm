//! linux-dmabuf: GPU buffers.
//!
//! GPU clients (Firefox, mpv, alacritty, anything using EGL/Vulkan) render into GPU memory
//! and share it as a DMA-BUF fd. We import it as a texture without any CPU copy. Before
//! accepting a buffer we try importing it, so a client can't make us fail at draw time.

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::ImportDma},
    wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};

use crate::state::SpaceState;

impl DmabufHandler for SpaceState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        self.backend.dmabuf_state()
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        match self.backend.renderer().import_dmabuf(&dmabuf, None) {
            Ok(_) => {
                let _ = notifier.successful::<SpaceState>();
            }
            Err(err) => {
                tracing::debug!("rejecting dmabuf: {err}");
                notifier.failed();
            }
        }
    }
}
