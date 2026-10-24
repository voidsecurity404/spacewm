//! The live block world: terrain + edits, and the background mesher.
//!
//! Chunks that changed are copied (with a one-block border) and sent to a worker thread,
//! which builds their meshes. Finished meshes come back through a calloop channel, so the
//! event loop wakes up and the main thread uploads them to the GPU — a few per frame, so a
//! burst of work never causes a visible stutter.

use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
};

use glam::{IVec3, Vec3};
use smithay::reexports::calloop::{channel, LoopHandle};

use super::{
    block::Block,
    chunk::{world_to_chunk, ChunkMap},
    mesh::{mesh_chunk, ChunkMeshData, PaddedChunk},
    terrain,
};
use crate::state::SpaceState;

pub struct VoxelWorld {
    pub map: ChunkMap,
    pub spawn: Vec3,
    pub spawn_yaw: f32,
    jobs: mpsc::Sender<PaddedChunk>,
    in_flight: HashSet<IVec3>,
    /// Meshes back from the worker, waiting for upload.
    pub pending: Vec<ChunkMeshData>,
    /// What the hotbar has selected for placing.
    pub selected: Block,
}

impl VoxelWorld {
    /// Generate the terrain, apply the saved edits and start the mesher thread.
    pub fn new(
        seed: u64,
        edits: &HashMap<IVec3, Block>,
        loop_handle: &LoopHandle<'static, SpaceState>,
    ) -> anyhow::Result<Self> {
        let started = std::time::Instant::now();
        let terrain = terrain::generate(seed);
        let mut map = terrain.map;
        for (&pos, &block) in edits {
            map.set_block_raw(pos, block);
        }
        tracing::info!(
            seed,
            chunks = map.chunks.len(),
            edits = edits.len(),
            ms = started.elapsed().as_millis() as u64,
            "terrain generated"
        );

        let (jobs, job_rx) = mpsc::channel::<PaddedChunk>();
        let (done_tx, done_rx) = channel::channel::<ChunkMeshData>();
        std::thread::Builder::new().name("spacewm-mesher".into()).spawn(move || {
            // Ends when the compositor drops the job sender.
            while let Ok(job) = job_rx.recv() {
                if done_tx.send(mesh_chunk(&job)).is_err() {
                    break;
                }
            }
        })?;
        loop_handle
            .insert_source(done_rx, |event, _, state| {
                if let channel::Event::Msg(mesh) = event {
                    let voxels = &mut state.voxels;
                    voxels.in_flight.remove(&mesh.pos);
                    // Edited again while this mesh was being built: a newer one follows, and
                    // showing this stale one would make the edit flicker back for a frame.
                    let stale = voxels
                        .map
                        .chunks
                        .get(&mesh.pos)
                        .is_some_and(|c| c.dirty && c.version > mesh.version);
                    if !stale {
                        voxels.pending.push(mesh);
                    }
                    state.queue_redraw();
                }
            })
            .map_err(|e| anyhow::anyhow!("mesher channel: {}", e.error))?;

        Ok(Self {
            map,
            spawn: terrain.spawn,
            spawn_yaw: terrain.spawn_yaw,
            jobs,
            in_flight: HashSet::new(),
            pending: Vec::new(),
            selected: Block::Stone,
        })
    }

    /// Send dirty chunks to the mesher, nearest to `near` first (what you look at
    /// appears first after startup).
    pub fn dispatch_dirty(&mut self, near: Vec3) {
        let center = world_to_chunk(near.floor().as_ivec3());
        let mut dirty: Vec<IVec3> = self
            .map
            .chunks
            .values()
            .filter(|c| c.dirty && !self.in_flight.contains(&c.pos))
            .map(|c| c.pos)
            .collect();
        if dirty.is_empty() {
            return;
        }
        dirty.sort_by_key(|p| (*p - center).length_squared());
        for pos in dirty {
            let job = PaddedChunk::from_map(&self.map, pos);
            if let Some(chunk) = self.map.chunks.get_mut(&pos) {
                chunk.dirty = false;
            }
            if self.jobs.send(job).is_ok() {
                self.in_flight.insert(pos);
            }
        }
    }

    pub fn solid(&self, p: IVec3) -> bool {
        self.map.block_at(p).is_solid()
    }

    pub fn water(&self, p: IVec3) -> bool {
        self.map.block_at(p) == Block::Water
    }

    pub fn set_block(&mut self, p: IVec3, block: Block) {
        self.map.set_block(p, block);
    }
}
