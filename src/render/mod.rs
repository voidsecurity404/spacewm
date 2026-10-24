//! Rendering shared by all backends.
//!
//! Stage 1: the flat 2D path (smithay's Space renderer) plus cursor, drag icon and
//! screenshots. Stages 2-3 add the 3D scene and window panels.

pub mod atlas;
pub mod camera;
pub mod crosshair;
pub mod cursor;
pub mod frustum;
pub mod gl;
pub mod help;
pub mod hud;
pub mod menu;
pub mod notice;
pub mod panel_texture;
pub mod perf;
pub mod scene;
pub mod screenshot;
pub mod text;
pub mod view;
pub mod voxels;

use std::time::Duration;

use smithay::{
    backend::renderer::{
        damage::{Error as DamageError, OutputDamageTracker, RenderOutputResult},
        element::{
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            texture::TextureRenderElement,
            Kind,
        },
        ImportAll, ImportMem, Renderer, Texture,
    },
    desktop::{space::render_output, utils::send_frames_surface_tree, Space, Window},
    input::pointer::CursorImageStatus,
    output::Output,
    utils::{Logical, Point, Scale},
};

use smithay::{backend::renderer::element::AsRenderElements, wayland::shell::wlr_layer::Layer};

use crate::{cli::RenderMode, input::mode::InputMode, state::DndIcon};

smithay::backend::renderer::element::render_elements! {
    /// Every kind of thing we draw on an output, so one list can hold them all.
    pub SpaceRenderElement<R> where R: ImportAll + ImportMem;
    Surface = WaylandSurfaceRenderElement<R>,
    Memory = MemoryRenderBufferRenderElement<R>,
    Texture = TextureRenderElement<R::TextureId>,
}

/// Dark grey background behind windows in 2D mode.
pub const CLEAR_COLOR_2D: [f32; 4] = [0.1, 0.1, 0.12, 1.0];

/// Cursor and drag-and-drop icon, front-most first.
#[allow(clippy::too_many_arguments)]
pub fn pointer_elements<R>(
    renderer: &mut R,
    cursor_status: &mut CursorImageStatus,
    cursor_theme: &mut cursor::CursorThemeCache,
    dnd_icon: Option<&DndIcon>,
    pointer: Point<f64, Logical>,
    scale: Scale<f64>,
    time: Duration,
) -> Vec<SpaceRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    let mut elements =
        cursor::cursor_elements(renderer, cursor_status, cursor_theme, pointer, scale, time);
    if let Some(icon) = dnd_icon {
        let pos = (pointer + icon.offset.to_f64()).to_physical(scale).to_i32_round();
        elements.extend(
            render_elements_from_surface_tree(
                renderer,
                &icon.surface,
                pos,
                scale,
                1.0,
                Kind::Unspecified,
            )
            .into_iter()
            .map(SpaceRenderElement::Surface),
        );
    }
    elements
}

/// Everything the 2D overlay (cursor, crosshair, drag icon) depends on.
pub struct OverlayInputs<'a> {
    pub render_mode: RenderMode,
    pub mode: &'a InputMode,
    pub cursor_status: &'a mut CursorImageStatus,
    pub cursor_theme: &'a mut cursor::CursorThemeCache,
    pub crosshair: &'a mut crosshair::Crosshair,
    pub dnd_icon: Option<&'a DndIcon>,
    /// Pointer in global 2D coordinates.
    pub pointer: Point<f64, Logical>,
    pub output: &'a Output,
    pub space: &'a Space<Window>,
    pub scale: Scale<f64>,
    pub time: Duration,
    /// Message box on top of everything (config errors).
    pub notice: Option<&'a notice::Notice>,
    /// The launcher box and the HUD line, with their size in logical px.
    pub launcher: Option<&'a (MemoryRenderBuffer, (i32, i32))>,
    pub hud: Option<&'a (MemoryRenderBuffer, (i32, i32))>,
    /// A layer surface has the keyboard: show a 2D cursor instead of the 3D one.
    pub layer_focus: bool,
    /// The hotbar (3D Walk mode) and the open inventory, with their size in logical px.
    pub hotbar: Option<&'a crate::inventory::Image>,
    pub inventory: Option<&'a crate::inventory::Image>,
    /// The key help box (Mod+K), on top of everything.
    pub help: Option<&'a crate::inventory::Image>,
    /// Volume/brightness level after a media key.
    pub osd: Option<&'a crate::inventory::Image>,
    /// "Starting Firefox ..." at the top.
    pub launching: Option<&'a crate::inventory::Image>,
    /// "drag: move" under the crosshair.
    pub drag_hint: Option<&'a crate::inventory::Image>,
    /// The window menu and its top-left corner (output-relative).
    pub menu: Option<(&'a crate::inventory::Image, (i32, i32))>,
    /// A fullscreen window is shown: where to draw the 2D cursor (output-relative
    /// logical px). Also hides bars.
    pub fullscreen_cursor: Option<Point<f64, Logical>>,
}

/// 2D things drawn on top of the output, front-most first:
/// - a notice (config error), in every mode;
/// - 2D mode: the client's cursor and the drag-and-drop icon;
/// - 3D Walk mode: the crosshair at the screen center;
/// - 3D Window mode: nothing (the cursor is drawn ON the panel, in 3D).
pub fn overlay_elements<R>(renderer: &mut R, inputs: OverlayInputs<'_>) -> Vec<SpaceRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    let Some(geo) = inputs.space.output_geometry(inputs.output) else {
        return Vec::new();
    };
    let scale = inputs.scale;
    let three_d = inputs.render_mode == RenderMode::Scene3D;
    let mut elements = Vec::new();
    let boxed = |renderer: &mut R, buffer: &MemoryRenderBuffer, at: (i32, i32)| {
        let pos = Point::<f64, Logical>::from((at.0 as f64, at.1 as f64)).to_physical(scale);
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            pos,
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
        .map(SpaceRenderElement::Memory)
    };
    if let Some((buffer, size)) = inputs.drag_hint {
        let x = (geo.size.w - size.0) / 2;
        elements.extend(boxed(renderer, buffer, (x, geo.size.h / 2 + 16)));
    }
    if let Some(((buffer, _), at)) = inputs.menu {
        // Its cursor on top of it.
        elements.extend(pointer_elements(
            renderer,
            inputs.cursor_status,
            inputs.cursor_theme,
            None,
            inputs.pointer - geo.loc.to_f64(),
            scale,
            inputs.time,
        ));
        elements.extend(boxed(renderer, buffer, at));
    }
    if let Some((buffer, size)) = inputs.help {
        let x = (geo.size.w - size.0) / 2;
        let y = ((geo.size.h - size.1) / 2).max(0);
        elements.extend(boxed(renderer, buffer, (x, y)));
    }
    if let Some(notice) = inputs.notice {
        // Centered, a bit above the middle (below the crosshair would look odd).
        let x = (geo.size.w - notice.size.0) / 2;
        let y = ((geo.size.h - notice.size.1) / 2 - geo.size.h / 8).max(0);
        elements.extend(boxed(renderer, &notice.buffer, (x, y)));
    }
    if let Some((buffer, size)) = inputs.inventory {
        // Its cursor on top of it.
        elements.extend(pointer_elements(
            renderer,
            inputs.cursor_status,
            inputs.cursor_theme,
            None,
            inputs.pointer - geo.loc.to_f64(),
            scale,
            inputs.time,
        ));
        let at = crate::inventory::inventory_origin((geo.size.w, geo.size.h));
        debug_assert_eq!(*size, crate::inventory::inventory_size());
        elements.extend(boxed(renderer, buffer, at));
    }
    if let Some((buffer, size)) = inputs.launcher {
        let x = (geo.size.w - size.0) / 2;
        let y = (geo.size.h / 5).min((geo.size.h - size.1).max(0));
        elements.extend(boxed(renderer, buffer, (x, y)));
    }

    // Layer surfaces, front to back. In 2D the space renderer draws them itself.
    let map = smithay::desktop::layer_map_for_output(inputs.output);
    let zone = map.non_exclusive_zone();
    let layer_elements =
        |renderer: &mut R, layer: Layer| layer_elements(renderer, &map, &[layer], scale);
    if three_d {
        if inputs.layer_focus {
            // A launcher-like layer is open: an ordinary 2D cursor clicks it.
            elements.extend(pointer_elements(
                renderer,
                inputs.cursor_status,
                inputs.cursor_theme,
                None,
                inputs.pointer - geo.loc.to_f64(),
                scale,
                inputs.time,
            ));
        }
        elements.extend(layer_elements(renderer, Layer::Overlay));
        // A fullscreen window covers bars (top layer), like on any desktop; overlays
        // (notifications, lockers-to-be) stay on top.
        if inputs.fullscreen_cursor.is_none() {
            elements.extend(layer_elements(renderer, Layer::Top));
        }
    }
    if let Some((buffer, _)) = inputs.hud {
        // Inside the area bars leave free, so it never hides under waybar.
        elements.extend(boxed(renderer, buffer, (zone.loc.x + 12, zone.loc.y + 10)));
    }
    if let Some((buffer, size)) = inputs.launching {
        let x = zone.loc.x + (zone.size.w - size.0) / 2;
        elements.extend(boxed(renderer, buffer, (x, zone.loc.y + 10)));
    }
    if let Some((buffer, size)) = inputs.osd {
        // Low and centered, above the hotbar; also over a fullscreen window.
        let x = zone.loc.x + (zone.size.w - size.0) / 2;
        let y = zone.loc.y + (zone.size.h - size.1 - crate::desktop::osd::BOTTOM_GAP).max(0);
        elements.extend(boxed(renderer, buffer, (x, y)));
    }
    if let Some(image) = inputs.hotbar {
        let at = crate::inventory::hotbar_rect(zone.size, image);
        elements.extend(boxed(renderer, &image.0, (zone.loc.x + at.0, zone.loc.y + at.1)));
    }
    if three_d && inputs.fullscreen_cursor.is_none() {
        elements.extend(layer_elements(renderer, Layer::Bottom));
    }
    drop(map);
    // The crosshair only while actually walking.
    let covered = inputs.layer_focus
        || inputs.launcher.is_some()
        || inputs.inventory.is_some()
        || inputs.help.is_some()
        || inputs.menu.is_some();
    if !covered || !three_d {
        elements.extend(mode_overlay(renderer, inputs, Some(geo)));
    }
    elements
}

fn mode_overlay<R>(
    renderer: &mut R,
    inputs: OverlayInputs<'_>,
    geo: Option<smithay::utils::Rectangle<i32, Logical>>,
) -> Vec<SpaceRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    match (inputs.render_mode, inputs.mode) {
        (RenderMode::Flat2D, _) => {
            let origin = geo.map(|g| g.loc.to_f64()).unwrap_or_default();
            pointer_elements(
                renderer,
                inputs.cursor_status,
                inputs.cursor_theme,
                inputs.dnd_icon,
                inputs.pointer - origin,
                inputs.scale,
                inputs.time,
            )
        }
        (RenderMode::Scene3D, InputMode::Walk) => {
            let Some(geo) = geo else {
                return Vec::new();
            };
            let half = crosshair::SIZE as f64 / 2.0;
            let center = geo.size.to_f64().to_point().downscale(2.0) - Point::from((half, half));
            let pos = center.to_physical(inputs.scale);
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                pos,
                inputs.crosshair.buffer(),
                None,
                None,
                None,
                Kind::Unspecified,
            )
            .map(|e| vec![SpaceRenderElement::Memory(e)])
            .unwrap_or_default()
        }
        // Fullscreen: an ordinary cursor. Otherwise it is drawn ON the panel, in 3D.
        (RenderMode::Scene3D, InputMode::Window { .. }) => match inputs.fullscreen_cursor {
            Some(pos) => pointer_elements(
                renderer,
                inputs.cursor_status,
                inputs.cursor_theme,
                inputs.dnd_icon,
                pos,
                inputs.scale,
                inputs.time,
            ),
            None => Vec::new(),
        },
    }
}

/// Build `OverlayInputs` from the state's fields while the backend is borrowed mutably
/// (a macro, because a function taking `&mut SpaceState` would borrow the backend too).
#[macro_export]
macro_rules! overlay_inputs {
    ($state:expr, $output:expr, $scale:expr, $time:expr) => {
        $crate::render::OverlayInputs {
            render_mode: $state.render_mode,
            mode: &$state.mode,
            cursor_status: &mut $state.cursor_status,
            cursor_theme: &mut $state.cursor_theme,
            crosshair: &mut $state.crosshair,
            dnd_icon: $state.dnd_icon.as_ref(),
            pointer: $state.pointer_location,
            output: $output,
            space: &$state.space,
            scale: $scale,
            time: $time,
            notice: $state.notice.as_ref(),
            launcher: $state.launcher.as_ref().and_then(|l| l.image.as_ref()),
            hud: $state.hud.image.as_ref(),
            layer_focus: matches!($state.overlay, $crate::state::Overlay::Layer(_)),
            hotbar: $state.hotbar.image.as_ref(),
            inventory: $state.inventory.as_ref().and_then(|i| i.image.as_ref()),
            help: $state.help.as_ref(),
            osd: $state.desktop.osd.as_ref().map(|o| &o.image),
            launching: $state.desktop.launching.image.as_ref(),
            menu: $state.menu.as_ref().and_then(|m| m.image.as_ref().map(|i| (i, m.origin))),
            drag_hint: $state.drag_hint.as_ref().map(|(_, i)| i),
            fullscreen_cursor: $state.fullscreen.map(|_| $state.virtual_cursor),
        }
    };
}

/// Same idea for the 3D scene inputs.
#[macro_export]
macro_rules! scene_inputs {
    ($state:expr, $time:expr) => {
        $crate::render::view::SceneInputs {
            world: &mut $state.world,
            highlight: $state.highlight,
            cursor: match $state.mode {
                $crate::input::mode::InputMode::Window { panel } => {
                    Some($crate::render::view::PanelCursor {
                        panel,
                        at: $state.virtual_cursor,
                        status: &mut $state.cursor_status,
                        theme: &mut $state.cursor_theme,
                        time: $time,
                    })
                }
                $crate::input::mode::InputMode::Walk => None,
            },
            chunk_meshes: &mut $state.voxels.pending,
            outline: $state.target_block.map(|b| b.block),
        }
    };
}

/// Render one output in 2D: windows from the space with `overlay` drawn on top.
pub fn render_2d<'d, R>(
    renderer: &mut R,
    framebuffer: &mut R::Framebuffer<'_>,
    age: usize,
    output: &Output,
    space: &Space<Window>,
    overlay: &[SpaceRenderElement<R>],
    damage_tracker: &'d mut OutputDamageTracker,
) -> Result<RenderOutputResult<'d>, DamageError<R::Error>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    render_output(
        output,
        renderer,
        framebuffer,
        1.0,
        age,
        [space],
        overlay,
        damage_tracker,
        CLEAR_COLOR_2D,
    )
}

/// Render one output in 3D mode: the scene texture with `overlay` (cursor) on top.
/// Windows are not drawn yet (Stage 3 puts them into the world).
pub fn render_3d<'d, R>(
    renderer: &mut R,
    framebuffer: &mut R::Framebuffer<'_>,
    age: usize,
    elements: &[SpaceRenderElement<R>],
    damage_tracker: &'d mut OutputDamageTracker,
) -> Result<RenderOutputResult<'d>, DamageError<R::Error>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    let [r, g, b] = scene::SKY_COLOR;
    damage_tracker.render_output(renderer, framebuffer, age, elements, [r, g, b, 1.0])
}

/// Layer surfaces of the given layers (first = front-most), output-local positions.
pub fn layer_elements<R>(
    renderer: &mut R,
    map: &smithay::desktop::LayerMap,
    layers: &[Layer],
    scale: Scale<f64>,
) -> Vec<SpaceRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    layers
        .iter()
        .flat_map(|&layer| map.layers_on(layer).rev())
        .filter_map(|l| Some((l, map.layer_geometry(l)?)))
        .flat_map(|(l, g)| {
            AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
                l,
                renderer,
                g.loc.to_physical_precise_round(scale),
                scale,
                1.0,
            )
        })
        .map(SpaceRenderElement::Surface)
        .collect()
}

/// What a locked output shows: only the locker's surface, or — if the locker is gone or
/// hasn't drawn yet — a "locked" message (red when the locker died). Also the clear color.
pub fn lock_elements<R>(
    renderer: &mut R,
    lock: &crate::wayland::session_lock::LockState,
    output: &Output,
    scale: Scale<f64>,
) -> (Vec<SpaceRenderElement<R>>, [f32; 4])
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    if let Some(surface) = lock.surface_for(output) {
        let elements = render_elements_from_surface_tree(
            renderer,
            surface.wl_surface(),
            (0, 0),
            scale,
            1.0,
            Kind::Unspecified,
        )
        .into_iter()
        .map(SpaceRenderElement::Surface)
        .collect();
        return (elements, [0.0, 0.0, 0.0, 1.0]);
    }
    let clear = if lock.is_abandoned() { [0.55, 0.0, 0.0, 1.0] } else { [0.0, 0.0, 0.0, 1.0] };
    let mut elements = Vec::new();
    if let (Some((buffer, size)), Some(mode)) = (&lock.message, output.current_mode()) {
        let logical = mode.size.to_f64().to_logical(scale);
        let at = Point::<f64, Logical>::from((
            (logical.w - size.0 as f64) / 2.0,
            (logical.h - size.1 as f64) / 2.0,
        ));
        if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.to_physical(scale),
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        ) {
            elements.push(SpaceRenderElement::Memory(e));
        }
    }
    (elements, clear)
}

/// Draw `elements` over `clear` (the locked screen).
pub fn render_with_clear<'d, R>(
    renderer: &mut R,
    framebuffer: &mut R::Framebuffer<'_>,
    age: usize,
    elements: &[SpaceRenderElement<R>],
    damage_tracker: &'d mut OutputDamageTracker,
    clear: [f32; 4],
) -> Result<RenderOutputResult<'d>, DamageError<R::Error>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    damage_tracker.render_output(renderer, framebuffer, age, elements, clear)
}

/// Collect presentation-time feedback from everything that was on `output` this frame
/// (windows/panels, layer surfaces, the lock surface). The backend reports it as
/// presented once the frame is really on screen (DRM: at the vblank).
pub fn take_presentation_feedback(
    output: &Output,
    space: &Space<Window>,
    lock: &crate::wayland::session_lock::LockState,
) -> smithay::desktop::utils::OutputPresentationFeedback {
    use smithay::{
        desktop::utils::{take_presentation_feedback_surface_tree, OutputPresentationFeedback},
        reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind,
    };
    let mut feedback = OutputPresentationFeedback::new(output);
    let on_output = |_: &_, _: &_| Some(output.clone());
    let flags = |_: &_, _: &_| Kind::empty();
    if let Some(surface) = lock.surface_for(output) {
        take_presentation_feedback_surface_tree(
            surface.wl_surface(),
            &mut feedback,
            on_output,
            flags,
        );
        return feedback; // nothing else is visible while locked
    }
    for window in space.elements() {
        window.take_presentation_feedback(&mut feedback, on_output, flags);
    }
    for layer in smithay::desktop::layer_map_for_output(output).layers() {
        layer.take_presentation_feedback(&mut feedback, on_output, flags);
    }
    feedback
}

/// "Now" on the monotonic clock presentation-time uses.
pub fn monotonic_now() -> smithay::utils::Time<smithay::utils::Monotonic> {
    smithay::utils::Clock::<smithay::utils::Monotonic>::new().now()
}

/// Panels whose apps get only throttled frame callbacks: 3D panels out of view (or
/// hidden with Mod+H). They keep running, slowly, without drawing frames nobody sees.
pub fn hidden_panels(
    render_mode: RenderMode,
    world: &crate::world::World,
    visible: &std::collections::HashSet<crate::world::panel::PanelId>,
    fullscreen: Option<crate::world::panel::PanelId>,
) -> std::collections::HashSet<crate::world::panel::PanelId> {
    if render_mode != RenderMode::Scene3D {
        return Default::default();
    }
    let shown = |id: &crate::world::panel::PanelId| match fullscreen {
        // Fullscreen: only that window is on screen.
        Some(f) => *id == f,
        None => visible.contains(id),
    };
    world.panels().iter().map(|p| p.id).filter(|id| !shown(id)).collect()
}

/// The fullscreen window drawn flat over the output: its geometry at the output's
/// top-left corner, X11 menus on top of it.
pub fn fullscreen_elements<R>(
    renderer: &mut R,
    window: &Window,
    x11_children: &[crate::xwayland::X11Child],
    scale: Scale<f64>,
) -> Vec<SpaceRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    // The buffer origin is the geometry's top-left minus its offset (CSD shadows).
    let origin = Point::<i32, Logical>::default() - window.geometry().loc;
    let mut elements = Vec::new();
    for child in x11_children.iter().rev() {
        let at = (origin + child.offset).to_physical_precise_round(scale);
        elements.extend(
            AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
                &child.surface,
                renderer,
                at,
                scale,
                1.0,
            )
            .into_iter()
            .map(SpaceRenderElement::Surface),
        );
    }
    elements.extend(
        AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
            window,
            renderer,
            origin.to_physical_precise_round(scale),
            scale,
            1.0,
        )
        .into_iter()
        .map(SpaceRenderElement::Surface),
    );
    elements
}

/// Tell clients they may draw their next frame. Without frame callbacks a client draws
/// one frame and then waits forever — the classic "frozen window" beginner bug.
pub fn send_frame_callbacks(
    output: &Output,
    space: &Space<Window>,
    cursor_status: &CursorImageStatus,
    dnd_icon: Option<&DndIcon>,
    lock: &crate::wayland::session_lock::LockState,
    hidden: &std::collections::HashSet<crate::world::panel::PanelId>,
    time: Duration,
) {
    if let Some(surface) = lock.surface_for(output) {
        send_frames_surface_tree(surface.wl_surface(), output, time, None, |_, _| {
            Some(output.clone())
        });
    }
    // Throttle: a surface that is not visible still gets a callback at most this often,
    // so hidden clients keep running slowly instead of freezing.
    let throttle = Some(Duration::from_secs(1));
    for window in space.elements() {
        // `None` = "not shown on any output": smithay then only sends a callback once per
        // `throttle`.
        let shown = !hidden.contains(&crate::state::window_id(window));
        window.send_frame(output, time, throttle, |_, _| shown.then(|| output.clone()));
    }
    // Bars and launchers too, or they would draw one frame and freeze.
    for layer in smithay::desktop::layer_map_for_output(output).layers() {
        layer.send_frame(output, time, throttle, |_, _| Some(output.clone()));
    }
    if let CursorImageStatus::Surface(surface) = cursor_status {
        send_frames_surface_tree(surface, output, time, throttle, |_, _| Some(output.clone()));
    }
    if let Some(icon) = dnd_icon {
        send_frames_surface_tree(&icon.surface, output, time, throttle, |_, _| {
            Some(output.clone())
        });
    }
}

impl crate::state::SpaceState {
    /// Throttled frame callbacks for panels out of view, without rendering (see the
    /// timer in main). Shown windows get theirs from the frame they are drawn in.
    pub fn send_hidden_frame_callbacks(&mut self) {
        let hidden = hidden_panels(
            self.render_mode,
            &self.world,
            &self.view.visible_panels,
            self.fullscreen_panel(),
        );
        if hidden.is_empty() {
            return;
        }
        let time = self.start_time.elapsed();
        let throttle = Some(Duration::from_secs(1));
        for output in self.space.outputs() {
            for window in self.space.elements() {
                if hidden.contains(&crate::state::window_id(window)) {
                    window.send_frame(output, time, throttle, |_, _| None);
                }
            }
        }
    }
}

/// 2D windows of one output as render elements, front-most first (for backends that
/// render element lists themselves, like DRM).
pub fn space_elements<R>(
    renderer: &mut R,
    space: &Space<Window>,
    output: &Output,
    scale: Scale<f64>,
) -> Vec<SpaceRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    let origin = space.output_geometry(output).map(|g| g.loc).unwrap_or_default();
    let windows: Vec<Window> = space.elements().rev().cloned().collect();
    windows
        .iter()
        .flat_map(|w| {
            // The buffer origin: window position minus its geometry offset (CSD shadows).
            let loc = space.element_location(w).unwrap_or_default() - w.geometry().loc - origin;
            AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
                w,
                renderer,
                loc.to_physical_precise_round(scale),
                scale,
                1.0,
            )
        })
        .map(SpaceRenderElement::Surface)
        .collect()
}

/// Render `elements` into a fresh offscreen texture and read it back. Used for
/// screenshots on DRM, where the scan-out buffer is not ours to read after queueing.
pub fn capture_elements(
    renderer: &mut smithay::backend::renderer::glow::GlowRenderer,
    elements: &[SpaceRenderElement<smithay::backend::renderer::glow::GlowRenderer>],
    size: smithay::utils::Size<i32, smithay::utils::Physical>,
    scale: Scale<f64>,
    clear: [f32; 4],
) -> Result<screenshot::CapturedImage, String> {
    use smithay::backend::{
        allocator::Fourcc,
        renderer::{gles::GlesTexture, Bind, Offscreen},
    };
    let mut texture: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())
        .map_err(|e| e.to_string())?;
    let mut tracker = OutputDamageTracker::new(size, scale, smithay::utils::Transform::Normal);
    let pending = {
        let mut fb = renderer.bind(&mut texture).map_err(|e| e.to_string())?;
        tracker
            .render_output(renderer, &mut fb, 0, elements, clear)
            .map_err(|e| format!("{e:?}"))?;
        screenshot::start_capture(renderer, &fb, size, smithay::utils::Transform::Normal)?
    };
    screenshot::finish_capture(renderer, pending)
}
