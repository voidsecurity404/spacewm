//! wp-fractional-scale: tell apps the exact scale to render at (1.25, 1.5 ...) instead
//! of rounding up to 2 and having us scale the buffer down. We announce the output's
//! scale; on panels in 3D that is also what makes text 1:1 sharp at the spawn distance.

use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::{
        compositor::with_states,
        fractional_scale::{with_fractional_scale, FractionalScaleHandler},
    },
};

use crate::state::SpaceState;

impl FractionalScaleHandler for SpaceState {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale())
            .unwrap_or(1.0);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fs| fs.set_preferred_scale(scale));
        });
    }
}
