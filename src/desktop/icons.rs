//! App icons for the dock, from the freedesktop icon theme folders.
//!
//! A .desktop file names its icon ("firefox"); the picture is a PNG under
//! icons/hicolor/<size>/apps/ or in /usr/share/pixmaps. SVG-only icons are not drawn
//! (that needs a full SVG renderer); the dock shows a letter for those.

use std::path::{Path, PathBuf};

/// Theme sizes tried, best first for a ~48 px icon (bigger ones scale down nicely).
const SIZES: &[&str] =
    &["64x64", "96x96", "128x128", "48x48", "256x256", "192x192", "512x512", "32x32"];

/// Where icons can be.
fn icon_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/icons/hicolor"));
    }
    let data = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for d in data.split(':').filter(|d| !d.is_empty()) {
        dirs.push(Path::new(d).join("icons/hicolor"));
    }
    dirs
}

/// The PNG file for an icon name (or an absolute path from the .desktop file).
pub fn find(icon: &str) -> Option<PathBuf> {
    let p = Path::new(icon);
    if p.is_absolute() {
        return (p.extension().is_some_and(|e| e == "png") && p.is_file()).then(|| p.to_path_buf());
    }
    // A name never contains a path separator; a hand-made one could try "../".
    if icon.is_empty() || icon.contains('/') {
        return None;
    }
    let file = format!("{icon}.png");
    for dir in icon_dirs() {
        for size in SIZES {
            let candidate = dir.join(size).join("apps").join(&file);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    ["/usr/share/pixmaps", "/usr/local/share/pixmaps"]
        .iter()
        .map(|d| Path::new(d).join(&file))
        .find(|p| p.is_file())
}

/// Decode a PNG into straight RGBA8, any color type.
fn decode(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // Palette and low bit depths to 8 bit, 16 bit down to 8.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    let px = (w * h) as usize;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..px * 4].to_vec(),
        png::ColorType::Rgb => {
            buf[..px * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect()
        }
        png::ColorType::GrayscaleAlpha => {
            buf[..px * 2].chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect()
        }
        png::ColorType::Grayscale => buf[..px].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None, // EXPAND turned these into RGB(A)
    };
    Some((w, h, rgba))
}

/// Scale straight RGBA to `size`x`size` by averaging (box filter: good for shrinking)
/// and premultiply the alpha, ready for the canvas.
fn scale_premultiplied(w: u32, h: u32, rgba: &[u8], size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    for oy in 0..size {
        for ox in 0..size {
            let (x0, x1) = (ox * w / size, ((ox + 1) * w / size).max(ox * w / size + 1).min(w));
            let (y0, y1) = (oy * h / size, ((oy + 1) * h / size).max(oy * h / size + 1).min(h));
            let mut acc = [0f32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * w + x) * 4) as usize;
                    let a = rgba[i + 3] as f32 / 255.0;
                    // Average premultiplied colors: transparent pixels add no color.
                    acc[0] += rgba[i] as f32 * a;
                    acc[1] += rgba[i + 1] as f32 * a;
                    acc[2] += rgba[i + 2] as f32 * a;
                    acc[3] += rgba[i + 3] as f32;
                }
            }
            let n = ((x1 - x0) * (y1 - y0)).max(1) as f32;
            let o = ((oy * size + ox) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// An icon as premultiplied RGBA, `size` px square.
pub fn load(icon: &str, size: u32) -> Option<Vec<u8>> {
    let path = find(icon)?;
    let (w, h, rgba) = decode(&path)?;
    Some(scale_premultiplied(w, h, &rgba, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
        let file = std::fs::File::create(path).expect("create");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().expect("header").write_image_data(rgba).expect("data");
    }

    #[test]
    fn decodes_and_scales() {
        let dir = std::env::temp_dir().join(format!("spacewm-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        // 4x4: left half opaque red, right half transparent.
        let mut px = Vec::new();
        for _y in 0..4 {
            for x in 0..4 {
                px.extend_from_slice(if x < 2 { &[255, 0, 0, 255] } else { &[0, 255, 0, 0] });
            }
        }
        let path = dir.join("t.png");
        write_png(&path, 4, 4, &px);
        let (w, h, rgba) = decode(&path).expect("decode");
        assert_eq!((w, h), (4, 4));
        let small = scale_premultiplied(w, h, &rgba, 2);
        assert_eq!(&small[0..4], &[255, 0, 0, 255], "left: red");
        assert_eq!(&small[4..8], &[0, 0, 0, 0], "right: transparent, no green bleeding in");
        // Absolute paths work, names with slashes don't.
        assert_eq!(find(path.to_str().expect("utf8")), Some(path.clone()));
        assert_eq!(find("../../etc/passwd"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
