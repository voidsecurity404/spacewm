//! Wayland protocol handlers.
//!
//! One file per protocol. Each implements a smithay handler trait on `SpaceState`. smithay
//! then needs `Dispatch` impls that route every protocol request to those handlers; the
//! single `delegate_dispatch2!` below generates them for ALL protocols at once (older
//! smithay versions needed one `delegate_*!` macro per protocol).

pub mod compositor;
pub mod data_device;
pub mod decoration;
pub mod dmabuf;
pub mod idle;
pub mod layer_shell;
pub mod output;
pub mod screencopy;
pub mod seat;
pub mod session_lock;
pub mod shm;
pub mod socket;
pub mod xdg_shell;

use crate::state::SpaceState;

smithay::delegate_dispatch2!(SpaceState);
