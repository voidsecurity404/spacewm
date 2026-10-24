//! wlr-layer-shell: bars, launchers, notifications, wallpapers.
//!
//! A layer surface is not a window: the client asks to be glued to edges of an output
//! ("top edge, full width, 30 px high") on one of four layers: background, bottom, top,
//! overlay. It may reserve an *exclusive zone* (a bar keeps that strip for itself) and
//! ask for keyboard focus: never, on demand (when clicked), or exclusively (a launcher
//! takes all keys until it closes).
//!
//! spacewm draws layers as a flat 2D overlay on top of the 3D view, per output. Background
//! layers are not drawn (the world is our background). An exclusive layer takes the
//! keyboard and suspends walking; while it is open a normal 2D cursor clicks layers.

use smithay::{
    desktop::{layer_map_for_output, LayerSurface, PopupKind, WindowSurfaceType},
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    utils::SERIAL_COUNTER,
    wayland::{
        compositor::with_states,
        shell::{
            wlr_layer::{
                KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData,
                WlrLayerShellHandler, WlrLayerShellState,
            },
            xdg::PopupSurface,
        },
    },
};

use crate::{focus::KeyboardFocusTarget, state::SpaceState};

/// Marks a layer surface the user took the keyboard back from (Mod+Escape).
struct Dismissed;

impl WlrLayerShellHandler for SpaceState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned());
        let Some(output) = output else {
            // No monitor to put it on: tell the client right away.
            surface.send_close();
            return;
        };
        tracing::info!(namespace, ?layer, output = output.name(), "new layer surface");
        let mut map = layer_map_for_output(&output);
        if let Err(e) = map.map_layer(&LayerSurface::new(surface, namespace)) {
            tracing::warn!("cannot map layer surface: {e}");
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        // Menus of a bar (waybar's tray): drawn with their layer surface.
        if let Err(e) = self.popups.track_popup(PopupKind::Xdg(popup)) {
            tracing::warn!("failed to track layer popup: {e}");
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for output in outputs {
            let mut map = layer_map_for_output(&output);
            let found = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            if let Some(layer) = found {
                tracing::info!(namespace = layer.namespace(), "layer surface closed");
                map.unmap_layer(&layer);
                drop(map);
                if self.overlay_layer().is_some_and(|l| l == &layer) {
                    self.release_layer_focus();
                }
            }
        }
        self.queue_redraw();
    }
}

impl SpaceState {
    /// Called on every commit: layers need their first configure (with the size we give
    /// them) and a re-arrange whenever they change anchors/size/exclusive zone.
    pub fn layer_commit(&mut self, surface: &WlSurface) {
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for output in outputs {
            let mut map = layer_map_for_output(&output);
            let Some(layer) = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned()
            else {
                continue;
            };
            let initial_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<LayerSurfaceData>()
                    .and_then(|d| d.lock().ok().map(|d| d.initial_configure_sent))
                    .unwrap_or(true)
            });
            map.arrange();
            if !initial_sent {
                layer.layer_surface().send_configure();
            }
            drop(map);
            self.update_layer_focus(&layer);
            self.queue_redraw();
            return;
        }
    }

    /// Exclusive keyboard interactivity on the top/overlay layer: that surface gets the
    /// keyboard (a launcher, a lock-like overlay) until it closes or gives it up.
    fn update_layer_focus(&mut self, layer: &LayerSurface) {
        let wants = layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive
            && matches!(layer.layer(), Layer::Top | Layer::Overlay)
            && layer.user_data().get::<Dismissed>().is_none();
        let has = self.overlay_layer().is_some_and(|l| l == layer);
        if wants && !has {
            self.take_layer_focus(layer.clone());
        } else if !wants && has {
            self.release_layer_focus();
        }
    }

    /// Mod+Escape while a layer has the keyboard: take it back and don't hand it to that
    /// surface again (it stays visible; closing it is up to its app).
    pub fn dismiss_layer(&mut self) {
        if let Some(layer) = self.overlay_layer().cloned() {
            tracing::info!(namespace = layer.namespace(), "keyboard taken back from layer");
            layer.user_data().insert_if_missing(|| Dismissed);
            self.release_layer_focus();
        }
    }

    /// The layer surface holding the keyboard, if any.
    pub fn overlay_layer(&self) -> Option<&LayerSurface> {
        match &self.overlay {
            crate::state::Overlay::Layer(l) => Some(l),
            _ => None,
        }
    }

    pub fn take_layer_focus(&mut self, layer: LayerSurface) {
        tracing::info!(namespace = layer.namespace(), "layer takes the keyboard");
        self.overlay = crate::state::Overlay::Layer(layer.clone());
        // Walking stops; the 2D cursor starts in the middle of the screen.
        self.view.controls.clear();
        if let Some(geo) = self.primary_output_geometry() {
            self.pointer_location = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(
                self,
                Some(KeyboardFocusTarget::Layer(layer)),
                SERIAL_COUNTER.next_serial(),
            );
        }
        self.queue_redraw();
    }

    /// Give the keyboard back to whatever had it before (the panel in Window mode, the
    /// top window in 2D, nobody while walking).
    pub fn release_layer_focus(&mut self) {
        if !matches!(self.overlay, crate::state::Overlay::Layer(_)) {
            return;
        }
        self.overlay = crate::state::Overlay::None;
        self.restore_keyboard_focus();
        self.queue_redraw();
    }

    pub fn restore_keyboard_focus(&mut self) {
        let serial = SERIAL_COUNTER.next_serial();
        let target = match (self.render_mode, self.mode) {
            (crate::cli::RenderMode::Scene3D, crate::input::mode::InputMode::Window { panel }) => {
                self.world
                    .panels()
                    .iter()
                    .find(|p| p.id == panel)
                    .map(|p| KeyboardFocusTarget::Window(p.window.clone()))
            }
            (crate::cli::RenderMode::Scene3D, _) => None,
            (crate::cli::RenderMode::Flat2D, _) => self
                .space
                .elements()
                .rev()
                .find(|w| !w.x11_surface().is_some_and(|x| x.is_override_redirect()))
                .cloned()
                .map(KeyboardFocusTarget::Window),
        };
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, target, serial);
        }
    }
}
