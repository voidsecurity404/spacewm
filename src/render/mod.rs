//! Rendering shared by all backends.
//!
//! Stage 1: the flat 2D path (smithay's Space renderer) plus cursor, drag icon and
//! screenshots. Stages 2-3 add the 3D scene and window panels.

pub mod camera;
pub mod crosshair;
pub mod cursor;
pub mod gl;
pub mod panel_texture;
pub mod scene;
pub mod screenshot;
pub mod view;

use std::time::Duration;

use smithay::{
    backend::renderer::{
        damage::{Error as DamageError, OutputDamageTracker, RenderOutputResult},
        element::{
            memory::MemoryRenderBufferRenderElement,
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
        let pos = (pointer + icon.offset.to_f64())
            .to_physical(scale)
            .to_i32_round();
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
}

/// 2D things drawn on top of the output, front-most first:
/// - 2D mode: the client's cursor and the drag-and-drop icon;
/// - 3D Walk mode: the crosshair at the screen center;
/// - 3D Window mode: nothing (the cursor is drawn ON the panel, in 3D).
pub fn overlay_elements<R>(renderer: &mut R, inputs: OverlayInputs<'_>) -> Vec<SpaceRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    let geo = inputs.space.output_geometry(inputs.output);
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
        (RenderMode::Scene3D, InputMode::Window { .. }) => Vec::new(),
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

/// Tell clients they may draw their next frame. Without frame callbacks a client draws
/// one frame and then waits forever — the classic "frozen window" beginner bug.
pub fn send_frame_callbacks(
    output: &Output,
    space: &Space<Window>,
    cursor_status: &CursorImageStatus,
    dnd_icon: Option<&DndIcon>,
    time: Duration,
) {
    // Throttle: a surface that is not visible still gets a callback at most this often,
    // so hidden clients keep running slowly instead of freezing.
    let throttle = Some(Duration::from_secs(1));
    for window in space.elements() {
        window.send_frame(output, time, throttle, |_, _| Some(output.clone()));
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
