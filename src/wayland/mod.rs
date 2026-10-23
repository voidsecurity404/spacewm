//! Wayland protocol handlers (Stage 1).
//!
//! One file per protocol: wl_compositor, xdg_shell, wl_shm, wl_seat, wl_output,
//! data device (clipboard). Each implements a smithay handler trait on
//! `SpaceState` and registers it with the matching `delegate_*!` macro.
