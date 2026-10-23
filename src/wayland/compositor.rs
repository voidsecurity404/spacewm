//! wl_compositor / wl_subcompositor: surfaces and their contents.
//!
//! A wl_surface is a rectangle a client draws into. The client attaches a buffer (its
//! pixels), marks damage, and calls `commit` to make the new state current atomically.
//! Subsurfaces are child surfaces positioned relative to a parent (video players,
//! embedded widgets); together they form a window's surface tree.

use smithay::wayland::seat::WaylandFocus;
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, CompositorClientState, CompositorHandler,
            CompositorState,
        },
    },
};

use crate::state::{ClientState, SpaceState};

impl CompositorHandler for SpaceState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        match client.get_data::<ClientState>() {
            Some(data) => &data.compositor_state,
            // Every client is inserted with a ClientState (socket.rs), so this cannot
            // happen; a panic here would point at a bug in client insertion.
            None => unreachable!("client without ClientState"),
        }
    }

    /// Runs every time a client commits a surface — i.e. finished a frame.
    fn commit(&mut self, surface: &WlSurface) {
        // Import the new buffer (shm or dmabuf) into the renderer's surface state so it can
        // be drawn, and track buffer damage.
        on_commit_buffer_handler::<Self>(surface);

        // A synchronized subsurface's state only applies when its parent commits, so it is
        // handled as part of the parent's commit.
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self
                .space
                .elements()
                .find(|w| w.wl_surface().as_deref() == Some(&root))
            {
                // Updates the window's cached geometry/bounding box.
                window.on_commit();
            }
        }

        crate::wayland::xdg_shell::handle_commit(self, surface);
        self.mark_surface_dirty(surface);
        self.queue_redraw();
    }
}

impl BufferHandler for SpaceState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}
