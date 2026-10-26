//! A hand-made map ("dust": a desert town) instead of the block world.
//!
//! The map file (resources/maps/dust/map.txt) describes a grid of cells carved out of
//! solid walls. Here it becomes boxes: neighboring cells of the same kind are merged
//! into as few rectangles as possible ("greedy meshing": grow a rectangle right while
//! the cells match, then down while whole rows match), so a long corridor wall is one
//! box with 5 faces instead of dozens. Those boxes are both what you see (a mesh, one
//! draw call) and what you bump into (collision), so the two always agree.
//!
//! Textures are mapped in world space: a wall's texture continues across boxes
//! instead of restarting at every edge, which hides where boxes meet.

use glam::{IVec2, Vec3};

/// The map that ships with spacewm.
pub const DUST: &str = include_str!("../../resources/maps/dust/map.txt");

/// Height of one step of raised paving: low enough to walk up without jumping.
pub const LEVEL: f32 = 0.35;
/// The ground: boxes reach this far down so nothing shows a gap below.
const BASE: f32 = -0.5;

/// Texture layers, in the order the renderer loads them (render/map.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mat {
    Sandstone = 0,
    Plaster = 1,
    Trim = 2,
    Sand = 3,
    Paving = 4,
    Crate = 5,
    Door = 6,
    Metal = 7,
}

/// File names (without .jpg) of the texture layers, in `Mat` order.
pub const TEXTURES: [&str; 8] =
    ["sandstone", "plaster", "trim", "sand", "paving", "crate", "door", "metal"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Wall,
    Floor,
    Arch,
    Tunnel,
    LowWall,
    Crate,
    Crates,
    DoorNs,
    DoorEw,
    Shutter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    kind: Kind,
    /// The floor: sand or paving, and how many steps up.
    paving: bool,
    level: u8,
}

const WALL: Cell = Cell { kind: Kind::Wall, paving: false, level: 0 };

/// An axis-aligned box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn overlaps(&self, min: Vec3, max: Vec3) -> bool {
        self.min.x < max.x
            && self.max.x > min.x
            && self.min.y < max.y
            && self.max.y > min.y
            && self.min.z < max.z
            && self.max.z > min.z
    }

    /// Where a ray first enters the box: (distance, face normal). The "slab" method: the
    /// ray is inside the box where it is between both planes of all three axes at once.
    pub fn ray(&self, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3)> {
        let (mut t0, mut t1) = (f32::NEG_INFINITY, f32::INFINITY);
        let mut normal = Vec3::ZERO;
        for a in 0..3 {
            if dir[a].abs() < 1e-9 {
                if origin[a] < self.min[a] || origin[a] > self.max[a] {
                    return None;
                }
                continue;
            }
            let (mut near, mut far) =
                ((self.min[a] - origin[a]) / dir[a], (self.max[a] - origin[a]) / dir[a]);
            let mut n = Vec3::ZERO;
            n[a] = -dir[a].signum();
            if near > far {
                std::mem::swap(&mut near, &mut far);
            }
            if near > t0 {
                t0 = near;
                normal = n;
            }
            t1 = t1.min(far);
        }
        (t0 <= t1 && t1 >= 0.0).then_some((t0.max(0.0), normal))
    }
}

/// How a box face's texture coordinates are made.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Uv {
    /// From world position, this many metres per texture repeat.
    World(f32),
    /// The texture once per face (crates, doors).
    Face,
}

/// One vertex of the map mesh (see render/map.rs for the attribute layout).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MapVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// Direction of increasing u (for normal mapping).
    pub tangent: [f32; 3],
    pub uv: [f32; 2],
    pub layer: f32,
    /// Multiplied with the texture (a little color variety between walls).
    pub tint: [f32; 3],
    /// Height of the floor this face stands on: faces darken just above it (cheap
    /// "ambient occlusion" where walls meet the ground).
    pub base_y: f32,
}

#[derive(Default)]
pub struct MapMesh {
    pub vertices: Vec<MapVertex>,
    pub indices: Vec<u32>,
}

pub struct Map {
    pub solids: Vec<Aabb>,
    pub mesh: MapMesh,
    pub spawn: Vec3,
    pub spawn_yaw_deg: f32,
    /// Everything (for the sun's shadow camera).
    pub bounds: Aabb,
}

struct Grid {
    cols: i32,
    rows: i32,
    cell: f32,
    wall_h: f32,
    cells: Vec<Cell>,
}

impl Grid {
    fn get(&self, c: i32, r: i32) -> Cell {
        if c < 0 || r < 0 || c >= self.cols || r >= self.rows {
            return WALL;
        }
        self.cells[(r * self.cols + c) as usize]
    }

    fn set(&mut self, c: i32, r: i32, f: impl Fn(Cell) -> Cell) {
        if c >= 0 && r >= 0 && c < self.cols && r < self.rows {
            let i = (r * self.cols + c) as usize;
            self.cells[i] = f(self.cells[i]);
        }
    }

    /// World x/z of a cell's north-west corner (the grid is centered on the origin).
    fn corner(&self, c: i32, r: i32) -> (f32, f32) {
        ((c - self.cols / 2) as f32 * self.cell, (r - self.rows / 2) as f32 * self.cell)
    }

    fn floor_y(&self, cell: Cell) -> f32 {
        cell.level as f32 * LEVEL
    }
}

/// Rectangles (c0, r0, c1, r1 inclusive) covering every cell where `key` gives the same
/// Some value, merged greedily; with that value.
fn rects<K: PartialEq + Copy>(g: &Grid, key: impl Fn(Cell) -> Option<K>) -> Vec<(IVec2, IVec2, K)> {
    let mut used = vec![false; g.cells.len()];
    let mut out = Vec::new();
    for r in 0..g.rows {
        for c in 0..g.cols {
            let i = (r * g.cols + c) as usize;
            let Some(k) = key(g.cells[i]).filter(|_| !used[i]) else { continue };
            let same = |cc: i32, rr: i32, used: &[bool]| {
                cc < g.cols
                    && rr < g.rows
                    && !used[(rr * g.cols + cc) as usize]
                    && key(g.get(cc, rr)) == Some(k)
            };
            let mut c1 = c;
            while same(c1 + 1, r, &used) {
                c1 += 1;
            }
            let mut r1 = r;
            while (c..=c1).all(|cc| same(cc, r1 + 1, &used)) {
                r1 += 1;
            }
            for rr in r..=r1 {
                for cc in c..=c1 {
                    used[(rr * g.cols + cc) as usize] = true;
                }
            }
            out.push((IVec2::new(c, r), IVec2::new(c1, r1), k));
        }
    }
    out
}

/// A small stable hash, for picking wall heights and colors that vary but never change.
fn hash(a: i32, b: i32) -> u32 {
    let mut h = (a as u32).wrapping_mul(0x9E37_79B9) ^ (b as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^ (h >> 13)
}

fn parse(text: &str) -> Result<(Grid, (i32, i32, f32)), String> {
    let mut grid: Option<Grid> = None;
    let mut spawn = None;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("# ") || line == "#" {
            continue;
        }
        let w: Vec<&str> = line.split_whitespace().collect();
        let err = |m: &str| format!("map line {}: {m}: {line}", n + 1);
        let num = |i: usize| -> Result<i32, String> {
            w.get(i).and_then(|s| s.parse().ok()).ok_or_else(|| err("expected a number"))
        };
        match w[0] {
            "size" => {
                let (cols, rows) = (num(1)?, num(2)?);
                let cell: f32 =
                    w.get(3).and_then(|s| s.parse().ok()).ok_or_else(|| err("cell size"))?;
                let wall_h: f32 =
                    w.get(4).and_then(|s| s.parse().ok()).ok_or_else(|| err("wall height"))?;
                if !(4..=256).contains(&cols) || !(4..=256).contains(&rows) || cell <= 0.0 {
                    return Err(err("bad size"));
                }
                grid = Some(Grid {
                    cols,
                    rows,
                    cell,
                    wall_h,
                    cells: vec![WALL; (cols * rows) as usize],
                });
            }
            "fill" | "stairs" => {
                let g = grid.as_mut().ok_or_else(|| err("size must come first"))?;
                let (c0, r0, c1, r1) = (num(2)?, num(3)?, num(4)?, num(5)?);
                if w[0] == "stairs" {
                    let (a, b) = (num(6)?, num(7)?);
                    let along = |c: i32, r: i32| -> (i32, i32) {
                        match w[1] {
                            "x+" => (c - c0, c1 - c0),
                            "x-" => (c1 - c, c1 - c0),
                            "z+" => (r - r0, r1 - r0),
                            _ => (r1 - r, r1 - r0),
                        }
                    };
                    if !["x+", "x-", "z+", "z-"].contains(&w[1]) {
                        return Err(err("direction must be x+, x-, z+ or z-"));
                    }
                    for r in r0..=r1 {
                        for c in c0..=c1 {
                            let (i, n) = along(c, r);
                            let level = if n == 0 { a } else { a + (b - a) * i / n };
                            let level = level.clamp(0, 9) as u8;
                            g.set(c, r, |_| Cell { kind: Kind::Floor, paving: true, level });
                        }
                    }
                    continue;
                }
                let ch = w[1].chars().next().ok_or_else(|| err("missing cell char"))?;
                let apply = move |old: Cell| -> Cell {
                    let keep = |kind| Cell { kind, ..old };
                    match ch {
                        '#' => WALL,
                        '.' => Cell { kind: Kind::Floor, paving: false, level: 0 },
                        ',' => Cell { kind: Kind::Floor, paving: true, level: 0 },
                        '1'..='9' => {
                            Cell { kind: Kind::Floor, paving: true, level: ch as u8 - b'0' }
                        }
                        'A' => Cell { kind: Kind::Arch, ..floorish(old) },
                        'T' => Cell { kind: Kind::Tunnel, ..floorish(old) },
                        'c' => keep(Kind::Crate),
                        'C' => keep(Kind::Crates),
                        'w' => keep(Kind::LowWall),
                        'd' => keep(Kind::DoorNs),
                        'D' => keep(Kind::DoorEw),
                        'm' => keep(Kind::Shutter),
                        _ => old,
                    }
                };
                if !"#.,123456789ATcCwdDm".contains(ch) {
                    return Err(err("unknown cell char"));
                }
                for r in r0..=r1 {
                    for c in c0..=c1 {
                        g.set(c, r, apply);
                    }
                }
            }
            "spawn" => {
                let yaw: f32 = w.get(3).and_then(|s| s.parse().ok()).ok_or_else(|| err("yaw"))?;
                spawn = Some((num(1)?, num(2)?, yaw));
            }
            _ => return Err(err("unknown command")),
        }
    }
    let grid = grid.ok_or("the map has no size line")?;
    let center = (grid.cols / 2, grid.rows / 2, 0.0);
    Ok((grid, spawn.unwrap_or(center)))
}

/// A cell turned into something you can walk on keeps its floor if it had one.
fn floorish(old: Cell) -> Cell {
    if old.kind == Kind::Wall {
        Cell { kind: Kind::Floor, paving: false, level: 0 }
    } else {
        old
    }
}

struct Builder {
    mesh: MapMesh,
    solids: Vec<Aabb>,
}

impl Builder {
    /// A box: its 5 visible faces (never the bottom) and a collision box.
    fn add(&mut self, b: Aabb, mat: Mat, uv: Uv, tint: [f32; 3], base_y: f32) {
        self.solids.push(b);
        let (lo, hi) = (b.min, b.max);
        // Each face: its normal, its tangent (the direction u grows: the viewer's right)
        // and its 4 corners as seen from outside: bottom-left, bottom-right, top-right,
        // top-left. That order goes counter-clockwise, which makes it the front side.
        // A corner: `false` picks the low side of that axis, `true` the high one.
        let v = |x: bool, y: bool, z: bool| {
            Vec3::new(
                if x { hi.x } else { lo.x },
                if y { hi.y } else { lo.y },
                if z { hi.z } else { lo.z },
            )
        };
        let (f, t) = (false, true);
        let mut faces: Vec<(Vec3, Vec3, [Vec3; 4])> = vec![
            // +X (east), -X (west), +Z (south), -Z (north)
            (Vec3::X, Vec3::NEG_Z, [v(t, f, t), v(t, f, f), v(t, t, f), v(t, t, t)]),
            (Vec3::NEG_X, Vec3::Z, [v(f, f, f), v(f, f, t), v(f, t, t), v(f, t, f)]),
            (Vec3::Z, Vec3::X, [v(f, f, t), v(t, f, t), v(t, t, t), v(f, t, t)]),
            (Vec3::NEG_Z, Vec3::NEG_X, [v(t, f, f), v(f, f, f), v(f, t, f), v(t, t, f)]),
            // +Y (top): the bottom edge of the texture is the south side.
            (Vec3::Y, Vec3::X, [v(f, t, t), v(t, t, t), v(t, t, f), v(f, t, f)]),
        ];
        if lo.y > 0.05 {
            // -Y, seen from below: the texture's up is south.
            faces.push((Vec3::NEG_Y, Vec3::X, [v(f, f, f), v(t, f, f), v(t, f, t), v(f, f, t)]));
        }
        for (normal, tangent, corners) in faces {
            let start = self.mesh.vertices.len() as u32;
            let up = normal.cross(tangent); // v grows along -up: down the texture
            for (i, p) in corners.iter().enumerate() {
                let uv = match uv {
                    Uv::World(scale) => [p.dot(tangent) / scale, -p.dot(up) / scale],
                    Uv::Face => {
                        [if i == 1 || i == 2 { 1.0 } else { 0.0 }, if i >= 2 { 0.0 } else { 1.0 }]
                    }
                };
                self.mesh.vertices.push(MapVertex {
                    pos: p.to_array(),
                    normal: normal.to_array(),
                    tangent: tangent.to_array(),
                    uv,
                    layer: mat as u8 as f32,
                    tint,
                    // Tops don't darken; walls darken just above the floor they meet.
                    base_y: if normal.y > 0.5 { -1000.0 } else { base_y },
                });
            }
            // Two triangles, both counter-clockwise like the corners.
            self.mesh.indices.extend([start, start + 1, start + 2, start, start + 2, start + 3]);
        }
    }
}

impl Map {
    pub fn load(text: &str) -> Result<Self, String> {
        let (g, (sc, sr, yaw)) = parse(text)?;
        let mut b = Builder { mesh: MapMesh::default(), solids: Vec::new() };
        let s = g.cell;
        let rect_box = |lo: IVec2, hi: IVec2, y0: f32, y1: f32| {
            let (x0, z0) = g.corner(lo.x, lo.y);
            let (x1, z1) = g.corner(hi.x + 1, hi.y + 1);
            Aabb { min: Vec3::new(x0, y0, z0), max: Vec3::new(x1, y1, z1) }
        };
        // The photos are a bit dark and grey for a sunny desert town: warm them up.
        const WARM: [f32; 3] = [1.12, 1.04, 0.90];

        // Floors: one box per merged area of the same material and height.
        let floors = rects(&g, |c| (c.kind != Kind::Wall).then_some((c.paving, c.level)));
        for (lo, hi, (paving, level)) in floors {
            let top = level as f32 * LEVEL;
            let (mat, scale, tint) = if paving {
                (Mat::Paving, 3.0, [1.12, 1.04, 0.86])
            } else {
                (Mat::Sand, 4.0, [1.04, 0.98, 0.90])
            };
            b.add(rect_box(lo, hi, BASE, top), mat, Uv::World(scale), tint, BASE);
        }

        // Walls: merged blocks, each a little different in height and color, with a
        // stone cornice on top that breaks up the skyline.
        for (lo, hi, ()) in rects(&g, |c| (c.kind == Kind::Wall).then_some(())) {
            let h = hash(lo.x * 7 + hi.x, lo.y * 13 + hi.y);
            let height = g.wall_h - 1.5 + (h % 4) as f32 * 0.75;
            let plaster = h % 10 < 3;
            let (mat, scale, base) = if plaster {
                (Mat::Plaster, 3.0, [1.42, 1.30, 1.08])
            } else {
                (Mat::Sandstone, 3.5, WARM)
            };
            let shade = 0.92 + ((h >> 8) % 10) as f32 * 0.016;
            let tint = [base[0] * shade, base[1] * shade, base[2] * shade];
            let wall = rect_box(lo, hi, BASE, height);
            b.add(wall, mat, Uv::World(scale), tint, 0.0);
            let grow = Vec3::new(0.12, 0.0, 0.12);
            let cornice = Aabb {
                min: Vec3::new(wall.min.x, height - 0.35, wall.min.z) - grow,
                max: Vec3::new(wall.max.x, height + 0.12, wall.max.z) + grow,
            };
            b.add(cornice, Mat::Trim, Uv::World(2.0), [0.95, 0.9, 0.82], height - 0.35);
        }

        // Archways and tunnels: something above the opening.
        for (lo, hi, kind) in
            rects(&g, |c| matches!(c.kind, Kind::Arch | Kind::Tunnel).then_some(c.kind))
        {
            let floor = g.floor_y(g.get(lo.x, lo.y));
            let (y0, mat, scale) = match kind {
                Kind::Arch => (floor + 4.0, Mat::Trim, 2.0),
                _ => (floor + 3.6, Mat::Sandstone, 3.5),
            };
            let top = g.wall_h - 0.75;
            b.add(rect_box(lo, hi, y0, top), mat, Uv::World(scale), WARM, -1000.0);
        }

        // Things standing in a cell.
        for r in 0..g.rows {
            for c in 0..g.cols {
                let cell = g.get(c, r);
                let floor = g.floor_y(cell);
                let (x0, z0) = g.corner(c, r);
                let center = Vec3::new(x0 + s / 2.0, floor, z0 + s / 2.0);
                let centered = |w: f32, h: f32, d: f32, y: f32| Aabb {
                    min: center + Vec3::new(-w / 2.0, y, -d / 2.0),
                    max: center + Vec3::new(w / 2.0, y + h, d / 2.0),
                };
                let wood = [0.95, 0.88, 0.75];
                match cell.kind {
                    Kind::Crate => {
                        b.add(centered(1.5, 1.5, 1.5, 0.0), Mat::Crate, Uv::Face, wood, floor)
                    }
                    Kind::Crates => {
                        b.add(centered(1.6, 1.6, 1.6, 0.0), Mat::Crate, Uv::Face, wood, floor);
                        b.add(
                            centered(1.2, 1.2, 1.2, 1.6),
                            Mat::Crate,
                            Uv::Face,
                            wood,
                            floor + 1.6,
                        );
                    }
                    Kind::LowWall => {
                        b.add(centered(s, 1.1, s, 0.0), Mat::Trim, Uv::World(2.0), WARM, floor)
                    }
                    Kind::DoorNs => b.add(
                        centered(s, 3.4, 0.22, 0.0),
                        Mat::Door,
                        Uv::World(1.7),
                        [0.75, 0.62, 0.45],
                        floor,
                    ),
                    Kind::Shutter => b.add(
                        centered(1.9, 2.6, 1.9, 0.0),
                        Mat::Metal,
                        Uv::World(2.0),
                        [0.55, 0.68, 0.72],
                        floor,
                    ),
                    Kind::DoorEw => b.add(
                        centered(0.22, 3.4, s, 0.0),
                        Mat::Door,
                        Uv::World(1.7),
                        [0.75, 0.62, 0.45],
                        floor,
                    ),
                    _ => {}
                }
            }
        }

        let (x0, z0) = g.corner(sc, sr);
        let spawn_cell = g.get(sc, sr);
        let spawn = Vec3::new(x0 + s / 2.0, g.floor_y(spawn_cell) + 0.05, z0 + s / 2.0);
        let (bx0, bz0) = g.corner(0, 0);
        let (bx1, bz1) = g.corner(g.cols, g.rows);
        let bounds =
            Aabb { min: Vec3::new(bx0, BASE, bz0), max: Vec3::new(bx1, g.wall_h + 2.0, bz1) };
        tracing::info!(boxes = b.solids.len(), triangles = b.mesh.indices.len() / 3, "map built");
        Ok(Map { solids: b.solids, mesh: b.mesh, spawn, spawn_yaw_deg: yaw, bounds })
    }

    /// Solid boxes overlapping min..max (the map is small: checking all is cheap).
    pub fn boxes(&self, min: Vec3, max: Vec3, out: &mut Vec<(Vec3, Vec3)>) {
        out.extend(self.solids.iter().filter(|b| b.overlaps(min, max)).map(|b| (b.min, b.max)));
    }

    /// The first solid a ray hits within `max`: (distance, face normal).
    pub fn ray(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<(f32, Vec3)> {
        self.solids
            .iter()
            .filter_map(|b| b.ray(origin, dir))
            .filter(|(t, _)| *t <= max)
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_map_builds() {
        let m = Map::load(DUST).expect("dust loads");
        // Merging keeps it small: a few hundred boxes, well under 20k triangles.
        assert!(m.solids.len() > 50 && m.solids.len() < 1500, "{} boxes", m.solids.len());
        assert!(m.mesh.indices.len() / 3 < 20_000, "{} triangles", m.mesh.indices.len() / 3);
        // The spawn stands on a floor, with room above.
        let feet = m.spawn;
        let mut out = Vec::new();
        m.boxes(feet - Vec3::new(0.3, 0.0, 0.3), feet + Vec3::new(0.3, 1.8, 0.3), &mut out);
        assert!(out.is_empty(), "spawn inside something: {out:?}");
        let down = m.ray(feet + Vec3::Y, Vec3::NEG_Y, 3.0).expect("floor below spawn");
        assert!((down.0 - 1.05).abs() < 0.01 && down.1 == Vec3::Y, "{down:?}");
    }

    #[test]
    fn greedy_merging_and_levels() {
        let m = Map::load("size 6 6 2 8\nfill . 1 1 4 4\nstairs x+ 1 2 4 2 1 4\nfill c 2 3 2 3\n")
            .expect("ok");
        // The open 4x4 area: sand rows 1, 3-4 and a row of 4 different stair levels.
        let floors = m.solids.iter().filter(|b| b.min.y == BASE && b.max.y < 2.0).count();
        assert!(floors <= 8, "{floors} floor boxes");
        // The top stair is 4 steps up.
        let top = m.solids.iter().map(|b| b.max.y).filter(|y| *y < 2.0).fold(0.0, f32::max);
        assert!((top - 4.0 * LEVEL).abs() < 1e-5 || top > 1.4, "top {top}");
    }

    #[test]
    fn rays_hit_the_near_face() {
        let b = Aabb { min: Vec3::new(1.0, 0.0, -1.0), max: Vec3::new(2.0, 2.0, 1.0) };
        let (t, n) = b.ray(Vec3::new(0.0, 1.0, 0.0), Vec3::X).expect("hit");
        assert!((t - 1.0).abs() < 1e-6 && n == Vec3::NEG_X);
        assert!(b.ray(Vec3::new(0.0, 3.0, 0.0), Vec3::X).is_none());
        assert!(b.ray(Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_X).is_none(), "behind");
    }

    #[test]
    fn bad_maps_say_where() {
        let e = Map::load("size 6 6 2 8\nfill ? 1 1 2 2\n").err().expect("error");
        assert!(e.contains("line 2"), "{e}");
        assert!(Map::load("fill . 1 1 2 2\n").is_err(), "size first");
    }
}
