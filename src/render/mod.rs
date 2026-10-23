//! Rendering shared by all backends.
//!
//! Stage 1: the flat 2D path (smithay's Space renderer) plus cursor, drag icon and
//! screenshots. Stages 2-3 add the 3D scene and window panels.

pub mod camera;
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

use crate::state::DndIcon;

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
