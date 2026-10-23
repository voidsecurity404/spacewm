//! Block textures, generated in code at startup (no image files, no copyrighted assets).
//!
//! Each block face texture is a 16×16 pixel-art tile: a base color plus per-pixel noise
//! and a few features (grass edge on dirt, wood rings, glass frame). The tiles go into a
//! GL *texture array* — one layer per tile — rather than one big atlas image: with an
//! atlas, mipmapping blends neighboring tiles together at the edges ("texture bleeding");
//! array layers are filtered independently.

use smithay::reexports::glow::{self, HasContext};

use crate::{error::SpaceError, render::gl::Gl, world::block::tile};

pub const TILE: i32 = 16;

fn hash(x: i32, y: i32, salt: u32) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD816_3841)
        ^ salt.wrapping_mul(0xCB1A_B31F);
    v ^= v >> 13;
    v = v.wrapping_mul(0x5BD1_E995);
    v ^= v >> 15;
    (v & 0xFFFF) as f32 / 65535.0
}

/// Straight (not premultiplied) RGBA in 0..1.
type Rgba = [f32; 4];

fn vary(c: [f32; 3], amount: f32, n: f32) -> Rgba {
    let k = 1.0 + (n - 0.5) * 2.0 * amount;
    [c[0] * k, c[1] * k, c[2] * k, 1.0]
}

fn pixel(layer: u16, x: i32, y: i32) -> Rgba {
    let n = hash(x, y, layer as u32);
    let n2 = hash(x / 2, y / 2, layer as u32 + 50);
    match layer {
        tile::GRASS_TOP => vary([0.36, 0.62, 0.28], 0.18, 0.6 * n + 0.4 * n2),
        tile::GRASS_SIDE => {
            // Dirt with a ragged strip of grass hanging over the top edge.
            let edge = 3 + (hash(x, 0, 77) * 2.5) as i32;
            if y < edge {
                vary([0.34, 0.60, 0.26], 0.15, n)
            } else {
                vary([0.47, 0.33, 0.22], 0.18, 0.5 * n + 0.5 * n2)
            }
        }
        tile::DIRT => vary([0.47, 0.33, 0.22], 0.18, 0.5 * n + 0.5 * n2),
        tile::STONE => {
            let speck = if n > 0.86 {
                0.75
            } else if n < 0.12 {
                1.2
            } else {
                1.0
            };
            let base = vary([0.50, 0.50, 0.52], 0.10, n2);
            [base[0] * speck, base[1] * speck, base[2] * speck, 1.0]
        }
        tile::WOOD_SIDE => {
            // Bark: vertical dark streaks.
            let streak = hash(x, 0, 91);
            let dark = if streak > 0.7 { 0.72 } else { 1.0 };
            let c = vary([0.40, 0.27, 0.15], 0.12, n);
            [c[0] * dark, c[1] * dark, c[2] * dark, 1.0]
        }
        tile::WOOD_TOP => {
            // Growth rings around the center.
            let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
            let ring = ((dx * dx + dy * dy).sqrt() * 1.1).floor() as i32 % 2;
            let base = if ring == 0 { [0.66, 0.50, 0.30] } else { [0.56, 0.41, 0.24] };
            if x == 0 || y == 0 || x == TILE - 1 || y == TILE - 1 {
                vary([0.40, 0.27, 0.15], 0.1, n)
            } else {
                vary(base, 0.06, n)
            }
        }
        tile::LEAVES => {
            // Holes for the cutout pass (alpha 0 = see-through).
            if n < 0.22 {
                [0.0, 0.0, 0.0, 0.0]
            } else {
                vary([0.22, 0.48, 0.18], 0.25, n2)
            }
        }
        tile::GLASS => {
            let border = x == 0 || y == 0 || x == TILE - 1 || y == TILE - 1;
            let streak = (x - y).abs() <= 1 && (3..9).contains(&x);
            if border {
                [0.85, 0.92, 0.96, 0.9]
            } else if streak {
                [0.95, 0.98, 1.0, 0.55]
            } else {
                [0.70, 0.85, 0.95, 0.25]
            }
        }
        tile::SAND => vary([0.86, 0.80, 0.56], 0.08, 0.7 * n + 0.3 * n2),
        tile::WATER => {
            let ripple = ((x as f32 * 0.7 + y as f32 * 0.35).sin() * 0.5 + 0.5) * 0.12;
            let c = vary([0.20, 0.42, 0.78], 0.06, n);
            [c[0] + ripple, c[1] + ripple, c[2] + ripple, 0.62]
        }
        _ => [1.0, 0.0, 1.0, 1.0], // magenta: "missing texture"
    }
}

/// RGBA8 bytes, premultiplied alpha, rows top-first, for one tile.
pub fn tile_pixels(layer: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity((TILE * TILE * 4) as usize);
    for y in 0..TILE {
        for x in 0..TILE {
            let [r, g, b, a] = pixel(layer, x, y);
            let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            out.extend_from_slice(&[to8(r * a), to8(g * a), to8(b * a), to8(a)]);
        }
    }
    out
}

/// Create the texture array and upload all tiles.
pub fn create_block_textures(gl: &Gl) -> Result<glow::Texture, SpaceError> {
    let layers = tile::COUNT as i32;
    // 16 → 8 → 4 → 2 → 1: five mip levels.
    let levels = 5;
    unsafe {
        let tex = gl.create_texture().map_err(SpaceError::Render)?;
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(tex));
        gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY, levels, glow::RGBA8, TILE, TILE, layers);
        for layer in 0..tile::COUNT {
            let data = tile_pixels(layer);
            gl.tex_sub_image_3d(
                glow::TEXTURE_2D_ARRAY,
                0,
                0,
                0,
                layer as i32,
                TILE,
                TILE,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&data)),
            );
        }
        gl.generate_mipmap(glow::TEXTURE_2D_ARRAY);
        // Up close: NEAREST keeps the crisp pixel-art look. Far away: blend between mip
        // levels so the ground doesn't shimmer when you move.
        gl.tex_parameter_i32(
            glow::TEXTURE_2D_ARRAY,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D_ARRAY,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST_MIPMAP_LINEAR as i32,
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        Ok(tex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_deterministic_and_sized() {
        for layer in 0..tile::COUNT {
            let a = tile_pixels(layer);
            assert_eq!(a.len(), (TILE * TILE * 4) as usize);
            assert_eq!(a, tile_pixels(layer));
        }
    }

    #[test]
    fn alpha_where_expected() {
        let alpha = |layer, x, y| tile_pixels(layer)[((y * TILE + x) * 4 + 3) as usize];
        assert_eq!(alpha(tile::STONE, 5, 5), 255);
        assert!(alpha(tile::GLASS, 8, 12) < 100, "glass is see-through");
        assert!(alpha(tile::WATER, 3, 3) < 200);
        let holes = (0..TILE * TILE)
            .filter(|i| tile_pixels(tile::LEAVES)[(i * 4 + 3) as usize] == 0)
            .count();
        assert!(holes > 20 && holes < 120, "leaves have some holes: {holes}");
    }

    #[test]
    fn premultiplied() {
        let px = tile_pixels(tile::GLASS);
        for c in px.chunks(4) {
            assert!(c[0] <= c[3] && c[1] <= c[3] && c[2] <= c[3]);
        }
    }
}
