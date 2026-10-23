//! Cursor images.
//!
//! Clients either draw their own cursor surface, or ask for a named shape ("text",
//! "pointer", ...) via wl_pointer/cursor-shape. Named shapes come from the user's xcursor
//! theme (XCURSOR_THEME / XCURSOR_SIZE). If the theme lacks a shape we fall back to the
//! theme's default arrow, and if there is no theme at all, to an arrow drawn in code.

use std::{collections::HashMap, io::Read, sync::Mutex, time::Duration};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::render_elements_from_surface_tree,
                Kind,
            },
            ImportAll, ImportMem, Renderer, Texture,
        },
    },
    input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Physical, Point, Scale, Transform},
    wayland::compositor::with_states,
};
use xcursor::{
    parser::{parse_xcursor, Image},
    CursorTheme,
};

use super::SpaceRenderElement;

/// One loaded cursor shape: all its sizes and animation frames.
struct LoadedCursor {
    images: Vec<Image>,
    /// GPU-ready buffers, one per image index, created on first use.
    buffers: HashMap<usize, MemoryRenderBuffer>,
}

pub struct CursorThemeCache {
    theme: CursorTheme,
    size: u32,
    shapes: HashMap<CursorIcon, Option<LoadedCursor>>,
    fallback: LoadedCursor,
}

impl CursorThemeCache {
    pub fn load() -> Self {
        let name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|s: &u32| (8..=256).contains(s))
            .unwrap_or(24);
        let theme = CursorTheme::load(&name);
        let fallback = load_shape(&theme, &["default", "left_ptr"]).unwrap_or_else(|| {
            tracing::warn!(theme = name, "no xcursor theme found, using built-in arrow");
            vec![builtin_arrow()]
        });
        tracing::debug!(theme = name, size, "cursor theme loaded");
        Self {
            theme,
            size,
            shapes: HashMap::new(),
            fallback: LoadedCursor {
                images: fallback,
                buffers: HashMap::new(),
            },
        }
    }

    /// The buffer and hotspot (in buffer pixels) to show for `icon` at `scale`.
    fn frame(
        &mut self,
        icon: CursorIcon,
        scale: u32,
        time: Duration,
    ) -> (MemoryRenderBuffer, Point<i32, Physical>) {
        let theme = &self.theme;
        let entry = self.shapes.entry(icon).or_insert_with(|| {
            let mut names = vec![icon.name()];
            names.extend_from_slice(icon.alt_names());
            load_shape(theme, &names).map(|images| LoadedCursor {
                images,
                buffers: HashMap::new(),
            })
        });
        let cursor = entry.as_mut().unwrap_or(&mut self.fallback);
        let index = frame_index(&cursor.images, self.size * scale, time.as_millis() as u32);
        let image = &cursor.images[index];
        let hotspot = Point::from((image.xhot as i32, image.yhot as i32));
        let buffer = cursor
            .buffers
            .entry(index)
            .or_insert_with(|| {
                MemoryRenderBuffer::from_slice(
                    &image.pixels_rgba,
                    // xcursor pixels are R,G,B,A bytes; Abgr8888 is that byte order.
                    Fourcc::Abgr8888,
                    (image.width as i32, image.height as i32),
                    1,
                    Transform::Normal,
                    None,
                )
            })
            .clone();
        (buffer, hotspot)
    }
}

/// Load the first of `names` that the theme has.
fn load_shape(theme: &CursorTheme, names: &[&str]) -> Option<Vec<Image>> {
    names.iter().find_map(|name| {
        let path = theme.load_icon(name)?;
        let mut data = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .read_to_end(&mut data)
            .ok()?;
        let images = parse_xcursor(&data)?;
        (!images.is_empty()).then_some(images)
    })
}

/// Pick the image nearest to `size`, and the animation frame for `millis`.
fn frame_index(images: &[Image], size: u32, millis: u32) -> usize {
    let nearest = images
        .iter()
        .min_by_key(|img| (size as i64 - img.size as i64).abs())
        .map(|img| img.size)
        .unwrap_or(0);
    let frames: Vec<usize> = images
        .iter()
        .enumerate()
        .filter(|(_, img)| img.size == nearest)
        .map(|(i, _)| i)
        .collect();
    let total: u32 = frames.iter().map(|&i| images[i].delay).sum();
    if total == 0 {
        return frames.first().copied().unwrap_or(0);
    }
    let mut t = millis % total;
    for &i in &frames {
        if t < images[i].delay {
            return i;
        }
        t -= images[i].delay;
    }
    frames.first().copied().unwrap_or(0)
}

/// A classic 24 px arrow drawn in code: white fill, black outline, premultiplied RGBA.
fn builtin_arrow() -> Image {
    const W: u32 = 24;
    const H: u32 = 24;
    // Polygon of the arrow tip-at-(1,1), in pixel units.
    let poly: [(f32, f32); 7] = [
        (1.0, 1.0),
        (1.0, 18.0),
        (5.5, 14.0),
        (9.0, 21.5),
        (12.0, 20.0),
        (8.5, 13.0),
        (14.5, 13.0),
    ];
    let inside = |x: f32, y: f32| {
        let mut c = false;
        let mut j = poly.len() - 1;
        for i in 0..poly.len() {
            let (xi, yi) = poly[i];
            let (xj, yj) = poly[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                c = !c;
            }
            j = i;
        }
        c
    };
    let mut pixels = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let fill = inside(px, py);
            // Outline: outside the arrow but within ~1.2 px of it.
            let near = !fill
                && [
                    (-1.2, 0.0),
                    (1.2, 0.0),
                    (0.0, -1.2),
                    (0.0, 1.2),
                    (-0.9, -0.9),
                    (0.9, 0.9),
                ]
                .iter()
                .any(|(dx, dy)| inside(px + dx, py + dy));
            let i = ((y * W + x) * 4) as usize;
            if fill {
                pixels[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            } else if near {
                pixels[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
    }
    Image {
        size: 24,
        width: W,
        height: H,
        xhot: 1,
        yhot: 1,
        delay: 0,
        pixels_rgba: pixels,
        pixels_argb: vec![],
    }
}

/// Render elements for the cursor at `location` (logical, output-local).
pub fn cursor_elements<R>(
    renderer: &mut R,
    status: &mut CursorImageStatus,
    theme: &mut CursorThemeCache,
    location: Point<f64, Logical>,
    scale: Scale<f64>,
    time: Duration,
) -> Vec<SpaceRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    // A client's cursor surface dies with the client: go back to the default arrow.
    if let CursorImageStatus::Surface(surface) = status {
        if !surface.alive() {
            *status = CursorImageStatus::default_named();
        }
    }

    match status {
        CursorImageStatus::Hidden => Vec::new(),
        CursorImageStatus::Named(icon) => {
            let int_scale = scale.x.ceil().max(1.0) as u32;
            let (buffer, hotspot) = theme.frame(*icon, int_scale, time);
            // The xcursor image is in buffer pixels at `int_scale`; place its hotspot on
            // the pointer position.
            let pos =
                location.to_physical(scale) - hotspot.to_f64().upscale(scale.x / int_scale as f64);
            match MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                pos,
                &buffer,
                None,
                None,
                None,
                Kind::Cursor,
            ) {
                Ok(elem) => vec![SpaceRenderElement::Memory(elem)],
                Err(err) => {
                    tracing::warn!("failed to upload cursor: {err:?}");
                    Vec::new()
                }
            }
        }
        CursorImageStatus::Surface(surface) => {
            let hotspot = cursor_hotspot(surface);
            let pos = (location - hotspot.to_f64())
                .to_physical(scale)
                .to_i32_round();
            render_elements_from_surface_tree(renderer, surface, pos, scale, 1.0, Kind::Cursor)
                .into_iter()
                .map(SpaceRenderElement::Surface)
                .collect()
        }
    }
}

/// Where inside its cursor surface the client wants the pointer tip.
pub fn cursor_hotspot(surface: &WlSurface) -> Point<i32, Logical> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<Mutex<CursorImageAttributes>>()
            .and_then(|attrs| attrs.lock().ok().map(|a| a.hotspot))
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_arrow_has_tip_and_outline() {
        let img = builtin_arrow();
        let px = |x: u32, y: u32| {
            let i = ((y * img.width + x) * 4) as usize;
            [img.pixels_rgba[i], img.pixels_rgba[i + 3]]
        };
        assert_eq!(px(2, 4), [255, 255], "inside the arrow is opaque white");
        assert_eq!(px(0, 4)[1], 255, "left edge has the black outline");
        assert_eq!(px(20, 2), [0, 0], "far right of the tip is transparent");
    }

    #[test]
    fn frame_index_picks_nearest_size() {
        let mk = |size| Image {
            size,
            width: size,
            height: size,
            xhot: 0,
            yhot: 0,
            delay: 0,
            pixels_rgba: vec![],
            pixels_argb: vec![],
        };
        let images = vec![mk(24), mk(32), mk(48)];
        assert_eq!(frame_index(&images, 30, 0), 1);
        assert_eq!(frame_index(&images, 100, 0), 2);
    }
}
