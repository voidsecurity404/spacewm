//! The crosshair at the screen center in Walk mode: white lines with a dark outline so it
//! is visible on any background.

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};

/// Image size in px (odd, so there is a true center pixel).
pub const SIZE: i32 = 21;
const HALF: i32 = SIZE / 2;
/// Arm length from the center and the empty gap around the center.
const ARM: i32 = 9;
const GAP: i32 = 2;

#[derive(Default)]
pub struct Crosshair {
    buffer: Option<MemoryRenderBuffer>,
}

impl Crosshair {
    pub fn buffer(&mut self) -> &MemoryRenderBuffer {
        self.buffer.get_or_insert_with(|| {
            let pixels = draw();
            MemoryRenderBuffer::from_slice(
                &pixels,
                Fourcc::Abgr8888,
                (SIZE, SIZE),
                1,
                Transform::Normal,
                None,
            )
        })
    }
}

fn on_arm(x: i32, y: i32) -> bool {
    let (dx, dy) = (x - HALF, y - HALF);
    let horizontal = dy.abs() == 0 && dx.abs() > GAP && dx.abs() <= ARM;
    let vertical = dx.abs() == 0 && dy.abs() > GAP && dy.abs() <= ARM;
    horizontal || vertical
}

/// Premultiplied RGBA: 1 px white arms, 1 px semi-dark outline around them.
fn draw() -> Vec<u8> {
    let mut px = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = ((y * SIZE + x) * 4) as usize;
            if on_arm(x, y) {
                px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            } else if (-1..=1).any(|dy| (-1..=1).any(|dx| on_arm(x + dx, y + dy))) {
                // Black at 60% alpha (premultiplied: rgb stays 0).
                px[i..i + 4].copy_from_slice(&[0, 0, 0, 153]);
            }
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crosshair_shape() {
        let px = draw();
        let at = |x: i32, y: i32| px[((y * SIZE + x) * 4 + 3) as usize];
        assert_eq!(at(HALF, HALF), 0, "center is empty");
        assert_eq!(at(HALF + GAP + 1, HALF), 255, "arm starts after the gap");
        assert_eq!(at(HALF + GAP + 1, HALF + 1), 153, "outline next to the arm");
        assert_eq!(at(0, 0), 0, "corners transparent");
    }
}
