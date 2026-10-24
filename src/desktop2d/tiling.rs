//! Tiling: where the windows of a workspace go. Master and stack, like many tiling
//! window managers: one window fills the area; with more, the first takes the left
//! part and the others share the right column, top to bottom. Gaps between windows
//! and around the edges keep them apart.

/// A rectangle in logical px: x, y, width, height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// Space between windows and around the edges.
pub const GAP: i32 = 8;
/// The master window's share of the width.
pub const MASTER: f32 = 0.55;

/// Rectangles for `n` tiled windows in `area`, in tiling order.
pub fn tile(area: Rect, n: usize) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let inner = Rect {
        x: area.x + GAP,
        y: area.y + GAP,
        w: (area.w - 2 * GAP).max(1),
        h: (area.h - 2 * GAP).max(1),
    };
    if n == 1 {
        return vec![inner];
    }
    let master_w = ((inner.w - GAP) as f32 * MASTER).round() as i32;
    let stack_x = inner.x + master_w + GAP;
    let stack_w = (inner.x + inner.w - stack_x).max(1);
    let mut out = vec![Rect { x: inner.x, y: inner.y, w: master_w, h: inner.h }];
    let rows = (n - 1) as i32;
    let row_h = ((inner.h - GAP * (rows - 1)) / rows).max(1);
    for i in 0..rows {
        let y = inner.y + i * (row_h + GAP);
        // The last one takes the rounding remainder, so the column ends at the edge.
        let h = if i == rows - 1 { inner.y + inner.h - y } else { row_h };
        out.push(Rect { x: stack_x, y, w: stack_w, h });
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// The rectangle nearest to `from` in direction `dir` (by centers), if any.
pub fn neighbor(from: Rect, others: &[Rect], dir: Dir) -> Option<usize> {
    let (fx, fy) = from.center();
    others
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            let (x, y) = r.center();
            match dir {
                Dir::Left => x < fx,
                Dir::Right => x > fx,
                Dir::Up => y < fy,
                Dir::Down => y > fy,
            }
        })
        .min_by_key(|(_, r)| {
            let (x, y) = r.center();
            let (dx, dy) = ((x - fx).abs(), (y - fy).abs());
            // Mostly along the direction, a little sideways counts too. Near-ties (a few
            // px of rounding) go to the upper / left one.
            let cost = match dir {
                Dir::Left | Dir::Right => dx + 2 * dy,
                Dir::Up | Dir::Down => dy + 2 * dx,
            };
            (cost / 16, y, x)
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0, y: 30, w: 1280, h: 690 };

    fn no_overlap(rects: &[Rect]) -> bool {
        rects.iter().enumerate().all(|(i, a)| {
            rects.iter().skip(i + 1).all(|b| {
                a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y
            })
        })
    }

    #[test]
    fn one_fills_the_area_with_gaps() {
        assert_eq!(tile(AREA, 1), vec![Rect { x: 8, y: 38, w: 1264, h: 674 }]);
        assert!(tile(AREA, 0).is_empty());
    }

    #[test]
    fn master_and_stack() {
        for n in 2..8 {
            let r = tile(AREA, n);
            assert_eq!(r.len(), n);
            assert!(no_overlap(&r), "{n}: {r:?}");
            // All inside the area, the stack column reaches the right and bottom edges.
            for t in &r {
                assert!(t.x >= AREA.x && t.y >= AREA.y);
                assert!(
                    t.x + t.w <= AREA.x + AREA.w - GAP + 1
                        && t.y + t.h <= AREA.y + AREA.h - GAP + 1
                );
            }
            assert_eq!(r[n - 1].y + r[n - 1].h, AREA.y + AREA.h - GAP);
            assert_eq!(r[1].x + r[1].w, AREA.x + AREA.w - GAP);
            assert!(r[0].w > r[1].w, "master is wider");
        }
    }

    #[test]
    fn neighbors() {
        let r = tile(AREA, 3); // master left, two stacked right
        assert_eq!(neighbor(r[0], &r, Dir::Right), Some(1));
        assert_eq!(neighbor(r[1], &r, Dir::Down), Some(2));
        assert_eq!(neighbor(r[2], &r, Dir::Up), Some(1));
        assert_eq!(neighbor(r[2], &r, Dir::Left), Some(0));
        assert_eq!(neighbor(r[0], &r, Dir::Left), None);
    }
}
