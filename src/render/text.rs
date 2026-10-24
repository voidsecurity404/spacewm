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

/// One line of text with optional colors of its own (straight RGBA).
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub text: String,
    pub fg: Option<[f32; 4]>,
    /// Background of the whole row (a highlighted list entry).
    pub bg: Option<[f32; 4]>,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Self::default() }
    }
}

/// Render text lines into a box. `scale` is the output's integer scale, so text is sharp
/// on HiDPI screens.
pub fn render(lines: &[String], style: Style, scale: i32) -> Option<TextImage> {
    let lines: Vec<Line> = lines.iter().map(|l| Line::plain(l.clone())).collect();
    render_lines(&lines, style, scale, 0)
}

/// Like `render`, with per-line colors and at least `min_cols` characters wide (so a box
/// whose text changes as you type keeps its size).
pub fn render_lines(
    lines: &[Line],
    style: Style,
    scale: i32,
    min_cols: usize,
) -> Option<TextImage> {
    let font = font()?;
    let scale = scale.max(1);
    let px = style.size * scale as f32;
    let pad = style.padding * scale;
    let advance = font.metrics('M', px).advance_width.ceil().max(1.0) as i32;
    let line_metrics = font.horizontal_line_metrics(px)?;
    let ascent = line_metrics.ascent.ceil() as i32;
    let line_height = line_metrics.new_line_size.ceil() as i32;

    let max_chars = ((style.max_width * scale - 2 * pad) / advance).max(8) as usize;
    // Wrap each line on its own, keeping its colors on every piece.
    let rows: Vec<(String, &Line)> = lines
        .iter()
        .flat_map(|l| {
            wrap(std::slice::from_ref(&l.text), max_chars).into_iter().map(move |t| (t, l))
        })
        .collect();
    let cols =
        rows.iter().map(|(t, _)| t.chars().count()).max().unwrap_or(0).max(min_cols.min(max_chars));
    let width = (cols as i32 * advance + 2 * pad).max(1);
    let height = (rows.len() as i32 * line_height + 2 * pad).max(1);

    let premul = |c: [f32; 4]| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut cache: HashMap<char, (fontdue::Metrics, Vec<u8>)> = HashMap::new();
    let row_of =
        |y: i32| ((y - pad).max(0) / line_height).min(rows.len().max(1) as i32 - 1) as usize;
    // Background first: the box color, or a row's own color across the full width.
    for y in 0..height {
        let row_bg =
            rows.get(row_of(y)).and_then(|(_, l)| l.bg).filter(|_| y >= pad && y < height - pad);
        let bg = premul(row_bg.unwrap_or(style.bg));
        let px_bg: [u8; 4] = std::array::from_fn(|i| (bg[i] * 255.0).round() as u8);
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&px_bg);
        }
    }
    for (row, (text, line)) in rows.iter().enumerate() {
        let fg = premul(line.fg.unwrap_or(style.fg));
        let baseline = pad + row as i32 * line_height + ascent;
        for (col, ch) in text.chars().enumerate() {
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
                    let a = mask[(gy * m.width as i32 + gx) as usize] as f32 / 255.0;
                    let i = ((y * width + x) * 4) as usize;
                    // "Over": text on top of whatever background is there (premultiplied).
                    for c in 0..4 {
                        let dst = rgba[i + c] as f32 / 255.0;
                        rgba[i + c] = ((fg[c] * a + dst * (1.0 - a)) * 255.0).round() as u8;
                    }
                }
            }
        }
    }
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
