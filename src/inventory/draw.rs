//! Drawing the inventory and the hotbar on the CPU: a small RGBA canvas, block icons and
//! text, turned into one image smithay uploads like any other buffer.
//!
//! Block icons are the classic isometric cube: the top face and two side faces of the
//! block's own 16×16 textures, the sides darker so the cube looks lit from above. Each
//! icon pixel is mapped back to a texture pixel ("inverse mapping": for every pixel we
//! draw, ask which texel lands there), which keeps the pixel-art edges crisp.

use crate::{
    render::{
        atlas::{self, TILE},
        text::{self, Line, Style},
    },
    world::block::Block,
};

/// Premultiplied RGBA image.
pub struct Canvas {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

/// Straight (not premultiplied) color, 0..1.
pub type Color = [f32; 4];

impl Canvas {
    pub fn new(width: i32, height: i32) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        Self { width, height, rgba: vec![0; (width * height * 4) as usize] }
    }

    /// Draw one premultiplied pixel over what is there ("over" blending).
    pub fn put(&mut self, x: i32, y: i32, src: [f32; 4]) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let i = ((y * self.width + x) * 4) as usize;
        for c in 0..4 {
            let dst = self.rgba[i + c] as f32 / 255.0;
            self.rgba[i + c] = ((src[c] + dst * (1.0 - src[3])) * 255.0).round() as u8;
        }
    }

    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color) {
        let p = [color[0] * color[3], color[1] * color[3], color[2] * color[3], color[3]];
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, p);
            }
        }
    }

    /// A frame `t` px thick inside the rectangle.
    pub fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, color: Color) {
        self.fill(x, y, w, t, color);
        self.fill(x, y + h - t, w, t, color);
        self.fill(x, y + t, t, h - 2 * t, color);
        self.fill(x + w - t, y + t, t, h - 2 * t, color);
    }

    /// Minecraft-style sunken slot: dark top/left edge, light bottom/right edge.
    pub fn inset(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, fill: Color) {
        self.fill(x, y, w, h, fill);
        self.fill(x, y, w, t, [0.22, 0.22, 0.22, 1.0]);
        self.fill(x, y, t, h, [0.22, 0.22, 0.22, 1.0]);
        self.fill(x, y + h - t, w, t, [1.0, 1.0, 1.0, 1.0]);
        self.fill(x + w - t, y, t, h, [1.0, 1.0, 1.0, 1.0]);
    }

    /// Copy a premultiplied image on top at (x, y).
    pub fn blit(&mut self, src: &[u8], sw: i32, sh: i32, x: i32, y: i32) {
        for sy in 0..sh {
            for sx in 0..sw {
                let i = ((sy * sw + sx) * 4) as usize;
                let p: [f32; 4] = std::array::from_fn(|c| src[i + c] as f32 / 255.0);
                if p[3] > 0.0 {
                    self.put(x + sx, y + sy, p);
                }
            }
        }
    }

    /// One line of text with its top-left corner at (x, y); returns its size.
    pub fn text(
        &mut self,
        s: &str,
        size: f32,
        color: Color,
        x: i32,
        y: i32,
        scale: i32,
    ) -> (i32, i32) {
        let style = Style { size, fg: color, bg: [0.0; 4], padding: 0, max_width: 4000 };
        match text::render_lines(&[Line::plain(s)], style, scale, 0) {
            Some(img) => {
                self.blit(&img.rgba, img.width, img.height, x, y);
                (img.width, img.height)
            }
            None => (0, 0),
        }
    }

    /// How far (px) the point is outside a rounded rectangle; negative inside. The
    /// "signed distance" to the shape: one number that gives smooth edges (coverage =
    /// how much of the pixel is inside) and soft shadows (fade with the distance).
    fn rounded_distance(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let qx = (px - cx).abs() - (w / 2.0 - r);
        let qy = (py - cy).abs() - (h / 2.0 - r);
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        outside + qx.max(qy).min(0.0) - r
    }

    /// A filled rectangle with rounded corners and smooth (anti-aliased) edges.
    #[allow(clippy::too_many_arguments)]
    pub fn round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, color: Color) {
        self.round_gradient(x, y, w, h, r, color, color);
    }

    /// `round_rect` whose color goes from `top` to `bottom`.
    #[allow(clippy::too_many_arguments)]
    pub fn round_gradient(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        r: i32,
        top: Color,
        bottom: Color,
    ) {
        let (fx, fy, fw, fh, fr) = (x as f32, y as f32, w as f32, h as f32, r as f32);
        for yy in y.max(0)..(y + h).min(self.height) {
            let t = ((yy - y) as f32 + 0.5) / fh.max(1.0);
            let c: [f32; 4] = std::array::from_fn(|i| top[i] + (bottom[i] - top[i]) * t);
            for xx in x.max(0)..(x + w).min(self.width) {
                let d =
                    Self::rounded_distance(xx as f32 + 0.5, yy as f32 + 0.5, fx, fy, fw, fh, fr);
                let cover = (0.5 - d).clamp(0.0, 1.0);
                if cover > 0.0 {
                    let a = c[3] * cover;
                    self.put(xx, yy, [c[0] * a, c[1] * a, c[2] * a, a]);
                }
            }
        }
    }

    /// A rounded outline `t` px thick, inside the rectangle.
    #[allow(clippy::too_many_arguments)]
    pub fn round_frame(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, t: i32, color: Color) {
        let (fx, fy, fw, fh, fr, ft) = (x as f32, y as f32, w as f32, h as f32, r as f32, t as f32);
        for yy in y.max(0)..(y + h).min(self.height) {
            for xx in x.max(0)..(x + w).min(self.width) {
                let d =
                    Self::rounded_distance(xx as f32 + 0.5, yy as f32 + 0.5, fx, fy, fw, fh, fr);
                // Inside the outer edge and outside the inner one.
                let cover = (0.5 - d).clamp(0.0, 1.0) * (0.5 + d + ft).clamp(0.0, 1.0);
                if cover > 0.0 {
                    let a = color[3] * cover;
                    self.put(xx, yy, [color[0] * a, color[1] * a, color[2] * a, a]);
                }
            }
        }
    }

    /// A soft shadow under a rounded rectangle: dark right at its edge, fading out over
    /// `blur` px (drawn before the shape itself).
    #[allow(clippy::too_many_arguments)]
    pub fn shadow(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, blur: i32, alpha: f32) {
        let (fx, fy, fw, fh, fr, fb) =
            (x as f32, y as f32, w as f32, h as f32, r as f32, blur as f32);
        for yy in (y - blur).max(0)..(y + h + blur).min(self.height) {
            for xx in (x - blur).max(0)..(x + w + blur).min(self.width) {
                let d =
                    Self::rounded_distance(xx as f32 + 0.5, yy as f32 + 0.5, fx, fy, fw, fh, fr);
                let t = (1.0 - d.max(0.0) / fb.max(1.0)).clamp(0.0, 1.0);
                let a = alpha * t * t;
                if a > 0.003 {
                    self.put(xx, yy, [0.0, 0.0, 0.0, a]);
                }
            }
        }
    }

    /// An on/off switch, `h` px tall and twice as wide.
    pub fn switch(&mut self, x: i32, y: i32, h: i32, on: bool, accent: Color) {
        let w = h * 2 - h / 4;
        let track = if on { accent } else { [0.30, 0.32, 0.37, 1.0] };
        self.round_rect(x, y, w, h, h / 2, track);
        let k = h - 6.max(h / 5);
        let kx = if on { x + w - k - (h - k) / 2 } else { x + (h - k) / 2 };
        self.round_rect(kx, y + (h - k) / 2, k, k, k / 2, [0.97, 0.97, 0.98, 1.0]);
    }

    /// Width in px of a line of text, to center it.
    pub fn text_width(s: &str, size: f32, scale: i32) -> i32 {
        let style = Style { size, fg: [1.0; 4], bg: [0.0; 4], padding: 0, max_width: 4000 };
        text::render_lines(&[Line::plain(s)], style, scale, 0).map_or(0, |i| i.width)
    }
}

/// An isometric icon of `block`, `size` px square, premultiplied RGBA.
pub fn block_icon(block: Block, size: i32) -> Vec<u8> {
    let size = size.max(4);
    let [top, side, _] = block.props().tiles;
    let top_px = atlas::tile_pixels(top);
    let side_px = atlas::tile_pixels(side);
    let s = size as f32;
    let cx = s / 2.0;
    // The cube's outline: top rhombus corners and the two side faces below it.
    // A face is a parallelogram origin + u*a + v*b; solve for (u, v) per pixel.
    struct Face<'a> {
        o: (f32, f32),
        a: (f32, f32),
        b: (f32, f32),
        tex: &'a [u8],
        shade: f32,
    }
    let faces = [
        Face {
            o: (0.0, s * 0.25),
            a: (cx, -s * 0.25),
            b: (cx, s * 0.25),
            tex: &top_px,
            shade: 1.0,
        },
        Face {
            o: (0.0, s * 0.25),
            a: (cx, s * 0.25),
            b: (0.0, s * 0.5),
            tex: &side_px,
            shade: 0.8,
        },
        Face {
            o: (cx, s * 0.5),
            a: (cx, -s * 0.25),
            b: (0.0, s * 0.5),
            tex: &side_px,
            shade: 0.62,
        },
    ];
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            for f in &faces {
                let det = f.a.0 * f.b.1 - f.a.1 * f.b.0;
                let (dx, dy) = (p.0 - f.o.0, p.1 - f.o.1);
                let u = (dx * f.b.1 - dy * f.b.0) / det;
                let v = (f.a.0 * dy - f.a.1 * dx) / det;
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let tx = ((u * TILE as f32) as i32).min(TILE - 1);
                let ty = ((v * TILE as f32) as i32).min(TILE - 1);
                let ti = ((ty * TILE + tx) * 4) as usize;
                let i = ((y * size + x) * 4) as usize;
                for c in 0..3 {
                    out[i + c] = (f.tex[ti + c] as f32 * f.shade).round() as u8;
                }
                out[i + 3] = f.tex[ti + 3];
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_shapes_have_smooth_corners() {
        let mut c = Canvas::new(40, 40);
        c.round_rect(0, 0, 40, 40, 10, [1.0, 1.0, 1.0, 1.0]);
        let a = |c: &Canvas, x: i32, y: i32| c.rgba[((y * 40 + x) * 4 + 3) as usize];
        assert_eq!(a(&c, 20, 20), 255, "inside");
        assert_eq!(a(&c, 0, 0), 0, "the very corner is cut off");
        let edge = a(&c, 3, 2);
        assert!(edge > 0 && edge < 255, "partly covered corner pixel: {edge}");
        let mut s = Canvas::new(60, 60);
        s.shadow(20, 20, 20, 20, 4, 10, 0.5);
        assert!(a2(&s, 30, 18) > a2(&s, 30, 12), "shadow fades with distance");
        fn a2(c: &Canvas, x: i32, y: i32) -> u8 {
            c.rgba[((y * 60 + x) * 4 + 3) as usize]
        }
    }

    #[test]
    fn icon_has_cube_shape() {
        let size = 32;
        let icon = block_icon(Block::Stone, size);
        let alpha = |x: i32, y: i32| icon[((y * size + x) * 4 + 3) as usize];
        // Center is covered, corners (outside the hexagon) are empty.
        assert_eq!(alpha(16, 16), 255);
        assert_eq!(alpha(1, 1), 0);
        assert_eq!(alpha(30, 1), 0);
        assert_eq!(alpha(1, 30), 0);
        // Top face is brighter than the right face (lit from above).
        let lum = |x: i32, y: i32| {
            let i = ((y * size + x) * 4) as usize;
            icon[i] as u32 + icon[i + 1] as u32 + icon[i + 2] as u32
        };
        let top: u32 = (12..20).map(|x| lum(x, 8)).sum();
        let right: u32 = (22..28).map(|x| lum(x, 20)).sum();
        assert!(top / 8 > right / 6, "top {top} right {right}");
    }

    #[test]
    fn canvas_blends_over() {
        let mut c = Canvas::new(4, 4);
        c.fill(0, 0, 4, 4, [1.0, 0.0, 0.0, 1.0]);
        c.fill(1, 1, 1, 1, [0.0, 0.0, 1.0, 0.5]);
        let i = ((4 + 1) * 4) as usize;
        assert_eq!(&c.rgba[i..i + 4], &[128, 0, 128, 255]);
        // Out of bounds is ignored, not a panic.
        c.fill(-3, -3, 10, 10, [0.0, 1.0, 0.0, 1.0]);
    }
}
