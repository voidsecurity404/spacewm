//! Chunks: the world is cut into 16×16×16 cubes of blocks.
//!
//! Why chunks? A world of millions of blocks can't be one mesh: changing one block would
//! rebuild everything, and the GPU would draw everything even when looking at the sky.
//! A chunk is the unit of meshing (only dirty chunks are rebuilt), of culling (chunks
//! outside the view are skipped) and of storage.

use std::collections::HashMap;

use glam::{IVec3, UVec3};

use super::block::Block;

pub const CHUNK: i32 = 16;
pub const CHUNK_VOLUME: usize = (CHUNK * CHUNK * CHUNK) as usize;

/// Index inside a chunk: x fastest, then z, then y.
pub fn idx(x: u32, y: u32, z: u32) -> usize {
    (x + z * 16 + y * 256) as usize
}

pub struct Chunk {
    pub pos: IVec3,
    pub blocks: Box<[Block; CHUNK_VOLUME]>,
    /// Must be re-meshed.
    pub dirty: bool,
    /// Bumped on every change, so a mesh built from an older snapshot can be discarded.
    pub version: u64,
}

impl Chunk {
    pub fn new(pos: IVec3) -> Self {
        Self { pos, blocks: Box::new([Block::Air; CHUNK_VOLUME]), dirty: true, version: 0 }
    }

    pub fn get(&self, local: UVec3) -> Block {
        self.blocks[idx(local.x, local.y, local.z)]
    }

    pub fn set(&mut self, local: UVec3, block: Block) {
        self.blocks[idx(local.x, local.y, local.z)] = block;
        self.dirty = true;
        self.version += 1;
    }
}

/// Which chunk a world block position is in. `div_euclid` rounds toward −∞, so block −1
/// is in chunk −1 (plain `/` would say chunk 0 and put two blocks at "local 0").
pub fn world_to_chunk(world: IVec3) -> IVec3 {
    IVec3::new(world.x.div_euclid(CHUNK), world.y.div_euclid(CHUNK), world.z.div_euclid(CHUNK))
}

/// Position inside its chunk (0..16 on every axis, also for negative coordinates).
pub fn world_to_local(world: IVec3) -> UVec3 {
    UVec3::new(
        world.x.rem_euclid(CHUNK) as u32,
        world.y.rem_euclid(CHUNK) as u32,
        world.z.rem_euclid(CHUNK) as u32,
    )
}

#[derive(Default)]
pub struct ChunkMap {
    pub chunks: HashMap<IVec3, Chunk>,
}

impl ChunkMap {
    /// The block at a world position; Air where no chunk exists.
    pub fn block_at(&self, world: IVec3) -> Block {
        self.chunks
            .get(&world_to_chunk(world))
            .map(|c| c.get(world_to_local(world)))
            .unwrap_or(Block::Air)
    }

    /// Change a block. Neighbor chunks are marked dirty too when the block is on a chunk
    /// border, because their faces next to it may appear or disappear.
    pub fn set_block(&mut self, world: IVec3, block: Block) {
        let cpos = world_to_chunk(world);
        let local = world_to_local(world);
        self.chunks.entry(cpos).or_insert_with(|| Chunk::new(cpos)).set(local, block);
        for axis in 0..3 {
            let (l, step) = (
                local[axis],
                IVec3::from(match axis {
                    0 => [1, 0, 0],
                    1 => [0, 1, 0],
                    _ => [0, 0, 1],
                }),
            );
            let neighbor = if l == 0 {
                Some(cpos - step)
            } else if l == CHUNK as u32 - 1 {
                Some(cpos + step)
            } else {
                None
            };
            if let Some(n) = neighbor.and_then(|n| self.chunks.get_mut(&n)) {
                n.dirty = true;
                n.version += 1;
            }
        }
    }

    /// Fast path for terrain generation: no neighbor bookkeeping (everything is dirty).
    pub fn set_block_raw(&mut self, world: IVec3, block: Block) {
        let cpos = world_to_chunk(world);
        let chunk = self.chunks.entry(cpos).or_insert_with(|| Chunk::new(cpos));
        let l = world_to_local(world);
        chunk.blocks[idx(l.x, l.y, l.z)] = block;
    }

    /// The highest solid block in a column, if any (y between `min_y` and `max_y`).
    pub fn top_solid(&self, x: i32, z: i32, min_y: i32, max_y: i32) -> Option<i32> {
        (min_y..=max_y).rev().find(|&y| self.block_at(IVec3::new(x, y, z)).is_solid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates() {
        assert_eq!(world_to_chunk(IVec3::new(-1, 0, 15)), IVec3::new(-1, 0, 0));
        assert_eq!(world_to_local(IVec3::new(-1, 0, 15)), UVec3::new(15, 0, 15));
        assert_eq!(world_to_chunk(IVec3::new(-16, -17, 16)), IVec3::new(-1, -2, 1));
        assert_eq!(world_to_local(IVec3::new(-16, -17, 16)), UVec3::new(0, 15, 0));
    }

    #[test]
    fn index_layout() {
        assert_eq!(idx(0, 0, 0), 0);
        assert_eq!(idx(1, 0, 0), 1);
        assert_eq!(idx(0, 0, 1), 16);
        assert_eq!(idx(0, 1, 0), 256);
        assert_eq!(idx(15, 15, 15), 4095);
    }

    #[test]
    fn set_and_get_across_chunks() {
        let mut map = ChunkMap::default();
        map.set_block(IVec3::new(-1, 5, -1), Block::Stone);
        assert_eq!(map.block_at(IVec3::new(-1, 5, -1)), Block::Stone);
        assert_eq!(map.block_at(IVec3::new(0, 5, 0)), Block::Air);
        assert_eq!(map.block_at(IVec3::new(1000, 5, 0)), Block::Air, "missing chunk = air");
    }

    #[test]
    fn border_edits_dirty_the_neighbor() {
        let mut map = ChunkMap::default();
        map.set_block_raw(IVec3::new(16, 0, 0), Block::Stone); // creates chunk (1,0,0)
        map.set_block_raw(IVec3::new(15, 0, 0), Block::Stone); // creates chunk (0,0,0)
        for c in map.chunks.values_mut() {
            c.dirty = false;
        }
        map.set_block(IVec3::new(15, 0, 0), Block::Air);
        assert!(map.chunks[&IVec3::new(0, 0, 0)].dirty);
        assert!(map.chunks[&IVec3::new(1, 0, 0)].dirty, "the face of (16,0,0) is now visible");
        // An inner edit leaves the neighbor alone.
        for c in map.chunks.values_mut() {
            c.dirty = false;
        }
        map.set_block(IVec3::new(8, 0, 0), Block::Dirt);
        assert!(!map.chunks[&IVec3::new(1, 0, 0)].dirty);
    }
}
