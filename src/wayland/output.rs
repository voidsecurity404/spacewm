//! wl_output: monitors as clients see them.
//!
//! Clients use outputs to pick a buffer scale and to know which monitor they are on
//! (wl_surface.enter/leave). xdg-output adds logical position/size.

use smithay::{
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::DisplayHandle,
    utils::{Physical, Size, Transform},
    wayland::output::OutputHandler,
};

use crate::state::SpaceState;

impl OutputHandler for SpaceState {}

/// Create an output and its wl_output global.
pub fn create_output(
    dh: &DisplayHandle,
    name: &str,
    model: &str,
    size: Size<i32, Physical>,
    refresh_mhz: i32,
    transform: Transform,
) -> Output {
    let output = Output::new(
        name.to_string(),
        PhysicalProperties {
            // Unknown physical size: 0x0. Clients then don't try to compute DPI.
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "spacewm".into(),
            model: model.into(),
            serial_number: "Unknown".into(),
        },
    );
    let mode = Mode {
        size,
        refresh: refresh_mhz,
    };
    output.change_current_state(
        Some(mode),
        Some(transform),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    let _global = output.create_global::<SpaceState>(dh);
    tracing::info!(
        name,
        width = size.w,
        height = size.h,
        refresh_hz = refresh_mhz as f32 / 1000.0,
        "output created"
    );
    output
}

/// Update an output after its size changed (winit window resized).
pub fn resize_output(output: &Output, size: Size<i32, Physical>, refresh_mhz: i32) {
    let mode = Mode {
        size,
        refresh: refresh_mhz,
    };
    output.change_current_state(Some(mode), None, None, None);
    output.set_preferred(mode);
}
