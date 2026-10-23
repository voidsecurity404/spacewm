//! Text for 2D overlays (config errors, the HUD, the launcher), drawn on the CPU.
//!
//! Glyphs come from a bundled font (Noto Sans Mono, SIL Open Font License, see
//! resources/fonts/) rasterized by `fontdue` into coverage masks: for each pixel, how
//! much of it the glyph covers (0..255). Blending the text color over the background by
//! that coverage gives smooth (anti-aliased) letters. The result is one RGBA image that
//! smithay uploads and draws like any other buffer.

use std::{collections::HashMap, sync::OnceLock};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};

const FONT_BYTES: &[u8] = include_bytes!("../../resources/fonts/NotoSansMono-Regular.ttf");

fn font() -> Option<&'static fontdue::Font> {
    static FONT: OnceLock<Option<fontdue::Font>> = OnceLock::new();
    FONT.get_or_init(|| {
        match fontdue::Font::from_bytes(FONT_BYTES, fontdue::FontSettings::default()) {
            Ok(font) => Some(font),
            Err(e) => {
                tracing::error!("bundled font is broken: {e}");
                None
            }
        }
    })
    .as_ref()
}

/// Colors are straight (not premultiplied) RGBA in 0..1.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    /// Font size in logical px.
    pub size: f32,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    /// Space around the text in logical px.
    pub padding: i32,
    /// Lines longer than this (logical px) are wrapped.
    pub max_width: i32,
}

/// A rendered text block, premultiplied RGBA, `scale` image px per logical px.
pub struct TextImage {
    pub rgba: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub scale: i32,
}

impl TextImage {
    pub fn buffer(&self) -> MemoryRenderBuffer {
        MemoryRenderBuffer::from_slice(
            &self.rgba,
            // Bytes are R,G,B,A in memory, which DRM calls ABGR8888 (little endian).
            Fourcc::Abgr8888,
            (self.width, self.height),
            self.scale,
            Transform::Normal,
            None,
        )
    }
}

/// Break `lines` so none is wider than `max_chars` (monospace), at spaces when possible.
pub fn wrap(lines: &[String], max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(8);
    let mut out = Vec::new();
    for line in lines {
        let mut rest: Vec<char> = line.chars().collect();
        while rest.len() > max_chars {
            // Break at the last space that fits; a word longer than a line is cut.
            let cut = rest[..=max_chars].iter().rposition(|c| *c == ' ').filter(|&i| i > 0);
            let at = cut.unwrap_or(max_chars);
            out.push(rest[..at].iter().collect());
            rest.drain(..(if cut.is_some() { at + 1 } else { at }));
        }
        out.push(rest.into_iter().collect());
    }
    out
}

/// Render text lines into a box. `scale` is the output's integer scale, so text is sharp
/// on HiDPI screens.
pub fn render(lines: &[String], style: Style, scale: i32) -> Option<TextImage> {
    let font = font()?;
    let scale = scale.max(1);
    let px = style.size * scale as f32;
    let pad = style.padding * scale;
    let advance = font.metrics('M', px).advance_width.ceil().max(1.0) as i32;
    let line_metrics = font.horizontal_line_metrics(px)?;
    let ascent = line_metrics.ascent.ceil() as i32;
    let line_height = line_metrics.new_line_size.ceil() as i32;

    let max_chars = ((style.max_width * scale - 2 * pad) / advance).max(8) as usize;
    let lines = wrap(lines, max_chars);
    let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as i32;
    let width = (cols * advance + 2 * pad).max(1);
    let height = (lines.len() as i32 * line_height + 2 * pad).max(1);

    let premul = |c: [f32; 4]| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
    let (fg, bg) = (premul(style.fg), premul(style.bg));
    let mut coverage = vec![0u8; (width * height) as usize];
    let mut cache: HashMap<char, (fontdue::Metrics, Vec<u8>)> = HashMap::new();
    for (row, line) in lines.iter().enumerate() {
        let baseline = pad + row as i32 * line_height + ascent;
        for (col, ch) in line.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let (m, mask) = cache.entry(ch).or_insert_with(|| font.rasterize(ch, px));
            let x0 = pad + col as i32 * advance + m.xmin;
            // ymin is the bitmap's bottom relative to the baseline (up is positive).
            let y0 = baseline - m.height as i32 - m.ymin;
            for gy in 0..m.height as i32 {
                for gx in 0..m.width as i32 {
                    let (x, y) = (x0 + gx, y0 + gy);
                    if x < 0 || y < 0 || x >= width || y >= height {
                        continue;
                    }
                    let c = mask[(gy * m.width as i32 + gx) as usize];
                    let dst = &mut coverage[(y * width + x) as usize];
                    *dst = (*dst).max(c);
                }
            }
        }
    }
    let rgba = coverage
        .iter()
        .flat_map(|&c| {
            let a = c as f32 / 255.0;
            // "Over": text on top of the background, both premultiplied.
            std::array::from_fn::<u8, 4, _>(|i| {
                ((fg[i] * a + bg[i] * (1.0 - a)) * 255.0).round() as u8
            })
        })
        .collect();
    Some(TextImage { rgba, width, height, scale })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_spaces() {
        let lines = wrap(&["hello wonderful world".to_string()], 12);
        assert_eq!(lines, vec!["hello", "wonderful", "world"]);
        let long = wrap(&["abcdefghijklmnopqrstuvwxyz".to_string()], 10);
        assert_eq!(long, vec!["abcdefghij", "klmnopqrst", "uvwxyz"]);
    }

    #[test]
    fn renders_text_with_coverage() {
        let style = Style {
            size: 16.0,
            fg: [1.0; 4],
            bg: [0.0, 0.0, 0.0, 1.0],
            padding: 4,
            max_width: 800,
        };
        let img = render(&["Hi".to_string()], style, 1).expect("bundled font loads");
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        // Some pixels are fully background, some are (nearly) full text color.
        assert!(img.rgba.chunks(4).any(|p| p[0] == 0));
        assert!(img.rgba.chunks(4).any(|p| p[0] > 200));
        // HiDPI: twice the pixels.
        let big = render(&["Hi".to_string()], style, 2).expect("font");
        assert!(big.width >= img.width * 2 - 2 && big.scale == 2);
    }
}
