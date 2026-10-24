//! xdg-decoration: who draws the title bar, the app (client-side) or the compositor.
//!
//! spacewm draws no title bars at all: the panel border is the decoration, and a title bar
//! with minimize/maximize buttons means little in 3D. So we always ask for server-side
//! mode, which tells apps like foot to drop their own title bar. An app that insists on
//! drawing its own (it sends request_mode(client_side)) is allowed to.

use smithay::{
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    wayland::shell::xdg::{decoration::XdgDecorationHandler, ToplevelSurface},
};

use crate::state::SpaceState;

fn set_mode(toplevel: &ToplevelSurface, mode: Mode) {
    toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
    // Before the first configure the mode simply rides along with it.
    if toplevel.is_initial_configure_sent() {
        toplevel.send_pending_configure();
    }
}

impl XdgDecorationHandler for SpaceState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        set_mode(&toplevel, Mode::ServerSide);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        tracing::debug!(?mode, "decoration mode requested");
        set_mode(&toplevel, mode);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        set_mode(&toplevel, Mode::ServerSide);
    }
}
