//! Turning chunks into triangles.
//!
//! Face culling: a block face is only drawn when the neighbor on that side lets you see
//! it (air, water, glass, ...). Inside solid ground almost every face touches another
//! block, so a typical chunk drops from 6 faces per block to a few hundred faces total.
//!
//! Ambient occlusion: each face corner gets darker when the blocks around that corner
//! are solid — the classic "3 neighbors" rule. Corners in creases look shaded, which
//! gives depth for almost no cost.
//!
//! Packed vertices: 8 bytes each (position as u8 inside the chunk, face + corner, texture
//! layer, AO) instead of ~32 bytes of floats. Less VRAM, and less memory bandwidth every
//! time the GPU reads the vertices.
//!
//! Meshing runs on a worker thread. It only needs a COPY of the chunk plus a one-block
//! border from its neighbors (`PaddedChunk`), so it never touches the live world and
//! never touches GL (GL calls must stay on the thread that owns the context).

use glam::IVec3;
#[cfg(test)]
use glam::UVec3;

use super::{
    block::{Block, RenderKind},
    chunk::{world_to_chunk, ChunkMap, CHUNK},
};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VoxelVertex {
    /// x, y, z inside the chunk (0..=16), and face (bits 0-2) | corner (bits 3-4).
    pub pos: [u8; 4],
    /// Texture array layer.
    pub tile: u16,
    /// Ambient occlusion: 0 = darkest corner, 3 = open.
    pub ao: u8,
    pub _pad: u8,
}

#[derive(Default, Debug)]
pub struct MeshData {
    pub vertices: Vec<VoxelVertex>,
    pub indices: Vec<u32>,
}

/// Result of meshing one chunk.
#[derive(Debug)]
pub struct ChunkMeshData {
    pub pos: IVec3,
    pub version: u64,
    /// Opaque + cutout (leaves) faces: first pass, depth tested and written.
    pub opaque: MeshData,
    /// Glass and water: second pass, blended.
    pub translucent: MeshData,
}

const PAD: i32 = CHUNK + 2;

/// A chunk plus the one-block layer around it, copied out of the world.
pub struct PaddedChunk {
    pub pos: IVec3,
    pub version: u64,
    blocks: Vec<Block>,
}

impl PaddedChunk {
    pub fn from_map(map: &ChunkMap, pos: IVec3) -> Self {
        let mut blocks = vec![Block::Air; (PAD * PAD * PAD) as usize];
        let base = pos * CHUNK;
        let version = map.chunks.get(&pos).map(|c| c.version).unwrap_or(0);
        for y in -1..=CHUNK {
            for z in -1..=CHUNK {
                for x in -1..=CHUNK {
                    let local = IVec3::new(x, y, z);
                    let b = if (0..CHUNK).contains(&x)
                        && (0..CHUNK).contains(&y)
                        && (0..CHUNK).contains(&z)
                    {
                        map.chunks[&pos].get(local.as_uvec3())
                    } else {
                        let world = base + local;
                        // Only look into the neighbor chunk if it exists.
                        if map.chunks.contains_key(&world_to_chunk(world)) {
                            map.block_at(world)
                        } else {
                            Block::Air
                        }
                    };
                    blocks[Self::index(x, y, z)] = b;
                }
            }
        }
        Self { pos, version, blocks }
    }

    fn index(x: i32, y: i32, z: i32) -> usize {
        ((x + 1) + (z + 1) * PAD + (y + 1) * PAD * PAD) as usize
    }

    /// Block at chunk-local coordinates in −1..=16.
    pub fn get(&self, x: i32, y: i32, z: i32) -> Block {
        self.blocks[Self::index(x, y, z)]
    }

    #[cfg(test)]
    pub fn from_fn(pos: IVec3, f: impl Fn(i32, i32, i32) -> Block) -> Self {
        let mut blocks = vec![Block::Air; (PAD * PAD * PAD) as usize];
        for y in -1..=CHUNK {
            for z in -1..=CHUNK {
                for x in -1..=CHUNK {
                    blocks[Self::index(x, y, z)] = f(x, y, z);
                }
            }
        }
        Self { pos, version: 0, blocks }
    }
}

/// The six faces: normal, then the 4 corners in counter-clockwise order seen from
/// outside (bottom-left, bottom-right, top-right, top-left), so the outside is the front.
const FACES: [([i32; 3], [[u8; 3]; 4]); 6] = [
    ([1, 0, 0], [[1, 0, 1], [1, 0, 0], [1, 1, 0], [1, 1, 1]]), // +X
    ([-1, 0, 0], [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]]), // −X
    ([0, 1, 0], [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]]), // +Y (top)
    ([0, -1, 0], [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]), // −Y (bottom)
    ([0, 0, 1], [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]]), // +Z
    ([0, 0, -1], [[1, 0, 0], [0, 0, 0], [0, 1, 0], [1, 1, 0]]), // −Z
];

/// Does `neighbor` let you see the face of `block` that touches it?
fn face_visible(block: Block, neighbor: Block) -> bool {
    match block.props().render {
        RenderKind::Invisible => false,
        // Opaque/cutout faces show whenever the neighbor isn't fully opaque. Leaves next
        // to leaves are skipped ("fast" foliage) — otherwise trees cost 6× the faces.
        RenderKind::Opaque | RenderKind::Cutout => neighbor.is_transparent() && neighbor != block,
        // Glass/water: only against air or a DIFFERENT see-through block, so a lake is one
        // surface, not thousands of internal walls.
        RenderKind::Translucent => {
            neighbor == Block::Air || (neighbor.is_transparent() && neighbor != block)
        }
    }
}

fn occludes(b: Block) -> bool {
    !b.is_transparent()
}

/// AO for one corner: `side1`, `side2` are the blocks next to the corner along the face,
/// `corner` the diagonal one. Two solid sides fully occlude, whatever the corner says.
fn vertex_ao(side1: bool, side2: bool, corner: bool) -> u8 {
    if side1 && side2 {
        0
    } else {
        3 - side1 as u8 - side2 as u8 - corner as u8
    }
}

pub fn mesh_chunk(chunk: &PaddedChunk) -> ChunkMeshData {
    let mut opaque = MeshData::default();
    let mut translucent = MeshData::default();
    for y in 0..CHUNK {
        for z in 0..CHUNK {
            for x in 0..CHUNK {
                let block = chunk.get(x, y, z);
                if block == Block::Air {
                    continue;
                }
                let props = block.props();
                let out = if props.render == RenderKind::Translucent {
                    &mut translucent
                } else {
                    &mut opaque
                };
                for (face, (n, corners)) in FACES.iter().enumerate() {
                    let neighbor = chunk.get(x + n[0], y + n[1], z + n[2]);
                    if !face_visible(block, neighbor) {
                        continue;
                    }
                    let tile = match face {
                        2 => props.tiles[0],
                        3 => props.tiles[2],
                        _ => props.tiles[1],
                    };
                    // The two axes along the face.
                    let axis = n.iter().position(|&v| v != 0).unwrap_or(0);
                    let (t1, t2) = ((axis + 1) % 3, (axis + 2) % 3);
                    let mut ao = [3u8; 4];
                    for (i, c) in corners.iter().enumerate() {
                        // Step from the block, through the face, toward this corner.
                        let mut base = [x + n[0], y + n[1], z + n[2]];
                        let s1 = if c[t1] == 1 { 1 } else { -1 };
                        let s2 = if c[t2] == 1 { 1 } else { -1 };
                        let mut a = base;
                        a[t1] += s1;
                        let mut b = base;
                        b[t2] += s2;
                        base[t1] += s1;
                        base[t2] += s2;
                        let side1 = occludes(chunk.get(a[0], a[1], a[2]));
                        let side2 = occludes(chunk.get(b[0], b[1], b[2]));
                        let corner = occludes(chunk.get(base[0], base[1], base[2]));
                        ao[i] = vertex_ao(side1, side2, corner);
                    }
                    let first = out.vertices.len() as u32;
                    for (i, c) in corners.iter().enumerate() {
                        out.vertices.push(VoxelVertex {
                            pos: [
                                x as u8 + c[0],
                                y as u8 + c[1],
                                z as u8 + c[2],
                                face as u8 | ((i as u8) << 3),
                            ],
                            tile,
                            ao: ao[i],
                            _pad: 0,
                        });
                    }
                    // Split the quad along the diagonal whose corners are equally lit;
                    // otherwise the AO gradient shows a visible crease.
                    if ao[0] as u16 + ao[2] as u16 >= ao[1] as u16 + ao[3] as u16 {
                        out.indices.extend_from_slice(&[
                            first,
                            first + 1,
                            first + 2,
                            first,
                            first + 2,
                            first + 3,
                        ]);
                    } else {
                        out.indices.extend_from_slice(&[
                            first,
                            first + 1,
                            first + 3,
                            first + 1,
                            first + 2,
                            first + 3,
                        ]);
                    }
                }
            }
        }
    }
    ChunkMeshData { pos: chunk.pos, version: chunk.version, opaque, translucent }
}

/// Unpack a vertex position back to chunk-local coordinates (for tests and debugging).
#[cfg(test)]
fn vertex_pos(v: &VoxelVertex) -> UVec3 {
    UVec3::new(v.pos[0] as u32, v.pos[1] as u32, v.pos[2] as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single(b: Block) -> PaddedChunk {
        PaddedChunk::from_fn(
            IVec3::ZERO,
            |x, y, z| {
                if (x, y, z) == (5, 5, 5) {
                    b
                } else {
                    Block::Air
                }
            },
        )
    }

    #[test]
    fn lone_block_has_six_faces() {
        let m = mesh_chunk(&single(Block::Stone));
        assert_eq!(m.opaque.vertices.len(), 24);
        assert_eq!(m.opaque.indices.len(), 36);
        assert!(m.translucent.vertices.is_empty());
        // Every vertex lies on the block's corners.
        for v in &m.opaque.vertices {
            let p = vertex_pos(v);
            assert!((5..=6).contains(&p.x) && (5..=6).contains(&p.y) && (5..=6).contains(&p.z));
        }
        // Nothing around: fully lit.
        assert!(m.opaque.vertices.iter().all(|v| v.ao == 3));
    }

    #[test]
    fn touching_faces_are_culled() {
        let c = PaddedChunk::from_fn(IVec3::ZERO, |x, y, z| {
            if y == 5 && z == 5 && (x == 5 || x == 6) {
                Block::Stone
            } else {
                Block::Air
            }
        });
        let m = mesh_chunk(&c);
        assert_eq!(m.opaque.vertices.len(), 10 * 4, "2 blocks: 12 faces minus the 2 touching");
    }

    #[test]
    fn solid_chunk_only_shows_its_outside() {
        let c = PaddedChunk::from_fn(IVec3::ZERO, |x, y, z| {
            if (0..16).contains(&x) && (0..16).contains(&y) && (0..16).contains(&z) {
                Block::Stone
            } else {
                Block::Air
            }
        });
        let m = mesh_chunk(&c);
        assert_eq!(m.opaque.vertices.len() / 4, 6 * 16 * 16, "one face per outer block side");
    }

    #[test]
    fn water_is_one_surface() {
        let c = PaddedChunk::from_fn(
            IVec3::ZERO,
            |_, y, _| if y <= 3 { Block::Water } else { Block::Air },
        );
        let m = mesh_chunk(&c);
        // Only the top surface (the padding is water too, so no side walls).
        assert_eq!(m.translucent.vertices.len() / 4, 16 * 16);
        assert!(m.opaque.vertices.is_empty());
    }

    #[test]
    fn ao_darkens_creases() {
        assert_eq!(vertex_ao(false, false, false), 3);
        assert_eq!(vertex_ao(true, false, false), 2);
        assert_eq!(vertex_ao(true, true, false), 0);
        assert_eq!(vertex_ao(false, false, true), 2);
        // A block on the floor next to a wall: its top face corners touching the wall get
        // darker than the open ones.
        let c = PaddedChunk::from_fn(IVec3::ZERO, |x, y, z| {
            if (y == 0 && (0..8).contains(&x) && (0..8).contains(&z))
                || (x == 3 && y == 1 && z == 4)
            {
                Block::Stone
            } else {
                Block::Air
            }
        });
        let m = mesh_chunk(&c);
        let floor_top_next_to_wall = m
            .opaque
            .vertices
            .iter()
            .filter(|v| v.pos[3] & 7 == 2 && v.pos[1] == 1 && v.pos[0] == 3 && v.pos[2] == 4);
        assert!(floor_top_next_to_wall.clone().count() > 0);
        assert!(floor_top_next_to_wall.clone().all(|v| v.ao < 3), "corner at the wall is shaded");
    }

    #[test]
    fn vertex_is_eight_bytes() {
        assert_eq!(std::mem::size_of::<VoxelVertex>(), 8);
    }
}
