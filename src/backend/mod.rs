//! Backends: where frames are shown and where input comes from.
//!
//! - winit: spacewm as a window inside the current desktop (safe for development).
//! - headless: no screen at all; renders offscreen on the GPU's render node. Used by
//!   automated tests together with IPC input injection and screenshots.
//! - drm (Stage 6): drives monitors and input devices directly from a TTY.
//!
//! This is an enum rather than a trait object: smithay's backend types are generic and
//! each backend needs different calloop sources, so a `match` keeps things explicit.

pub mod drm;
pub mod headless;
pub mod winit;

use smithay::{
    backend::renderer::glow::GlowRenderer,
    input::keyboard::LedState,
    output::Output,
    reexports::{calloop::LoopHandle, wayland_server::DisplayHandle},
    wayland::dmabuf::DmabufState,
};

pub use crate::cli::BackendKind;
use crate::{error::SpaceError, state::SpaceState};

// One instance for the whole run, so the size difference between variants doesn't matter.
#[allow(clippy::large_enum_variant)]
pub enum Backend {
    Winit(winit::WinitBackend),
    Headless(headless::HeadlessBackend),
    Drm(drm::DrmBackend),
}

impl Backend {
    pub fn init(
        kind: BackendKind,
        cli: &crate::cli::Cli,
        dh: &DisplayHandle,
        loop_handle: &LoopHandle<'static, SpaceState>,
    ) -> Result<Self, SpaceError> {
        Ok(match kind {
            BackendKind::Winit => Self::Winit(winit::WinitBackend::init(dh, loop_handle)?),
            BackendKind::Headless => {
                Self::Headless(headless::HeadlessBackend::init(cli, dh, loop_handle)?)
            }
            BackendKind::Drm => Self::Drm(drm::DrmBackend::init(cli, dh, loop_handle)?),
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Winit(_) => "winit",
            Self::Headless(_) => "headless",
            Self::Drm(_) => "drm",
        }
    }

    /// Nested inside another desktop: the host eats Super, so binds also accept Alt.
    pub fn is_nested(&self) -> bool {
        matches!(self, Self::Winit(_))
    }

    pub fn renderer(&mut self) -> &mut GlowRenderer {
        match self {
            Self::Winit(b) => b.renderer(),
            Self::Headless(b) => b.renderer(),
            Self::Drm(b) => b.renderer(),
        }
    }

    pub fn dmabuf_state(&mut self) -> &mut DmabufState {
        match self {
            Self::Winit(b) => &mut b.dmabuf_state,
            Self::Headless(b) => &mut b.dmabuf_state,
            Self::Drm(b) => b.dmabuf_state(),
        }
    }

    pub fn outputs(&self) -> Vec<Output> {
        match self {
            Self::Winit(b) => vec![b.output.clone()],
            Self::Headless(b) => vec![b.output.clone()],
            Self::Drm(b) => b.outputs(),
        }
    }

    /// Something changed on screen: render a new frame as soon as the display allows.
    pub fn schedule_redraw(&mut self) {
        match self {
            Self::Winit(b) => b.schedule_redraw(),
            Self::Headless(b) => b.schedule_redraw(),
            Self::Drm(b) => b.schedule_redraw(),
        }
    }

    pub fn update_led_state(&mut self, led_state: LedState) {
        // Keyboard LEDs (Caps/Num Lock) only exist on real devices.
        if let Self::Drm(b) = self {
            b.update_led_state(led_state);
        }
    }

    /// Ctrl+Alt+Fn: switch to another VT (only meaningful on a real session).
    pub fn change_vt(&mut self, vt: i32) {
        if let Self::Drm(b) = self {
            b.change_vt(vt);
        }
    }
}
