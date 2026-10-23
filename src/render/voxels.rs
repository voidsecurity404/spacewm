//! Drawing the block world: chunk meshes on the GPU, culling, the two passes.

use std::collections::HashMap;

use glam::{IVec3, Mat4, Vec3};
use smithay::reexports::glow::{self, HasContext};

use super::{
    atlas,
    camera::Camera,
    frustum::Frustum,
    gl::{gl_check, Attrib, ColorVertex, Gl, Mesh, Program, Vertex},
};
use crate::{
    error::SpaceError,
    world::{
        chunk::CHUNK,
        mesh::{ChunkMeshData, VoxelVertex},
    },
};

impl Vertex for VoxelVertex {
    fn attribs() -> &'static [Attrib] {
        &[
            // Integer attributes: the shader receives uvec4/uint, unpacked there.
            Attrib {
                location: 0,
                components: 4,
                ty: glow::UNSIGNED_BYTE,
                normalized: false,
                offset: 0,
            },
            Attrib {
                location: 1,
                components: 1,
                ty: glow::UNSIGNED_SHORT,
                normalized: false,
                offset: 4,
            },
            Attrib {
                location: 2,
                components: 1,
                ty: glow::UNSIGNED_BYTE,
                normalized: false,
                offset: 6,
            },
        ]
    }
}

/// Direction toward the sun: high, slightly from the south-east, so the three visible
/// sides of a block get three different brightnesses.
const SUN_DIR: Vec3 = Vec3::new(0.35, 0.85, 0.40);

/// Default render distance in chunks.
pub const RENDER_DISTANCE: i32 = 8;
/// Chunk meshes uploaded per frame at most; more would cause visible stutter.
pub const UPLOADS_PER_FRAME: usize = 4;

struct ChunkGpu {
    opaque: Option<Mesh>,
    translucent: Option<Mesh>,
}

/// Which chunks to draw this frame.
#[derive(Default)]
pub struct VoxelFrame {
    pub opaque: Vec<IVec3>,
    /// Sorted far → near, so each one blends over what is behind it.
    pub translucent: Vec<IVec3>,
    /// The block the player is aiming at gets an outline.
    pub outline: Option<IVec3>,
}

pub struct VoxelRenderer {
    textures: glow::Texture,
    chunks: HashMap<IVec3, ChunkGpu>,
    outline: Mesh,
    pub uploads: u32,
}

fn outline_mesh(gl: &Gl) -> Result<Mesh, SpaceError> {
    // The 12 edges of a unit cube, drawn as lines, a hair larger than the block so they
    // don't z-fight with its faces.
    let c = [0.1, 0.1, 0.1];
    let p = |x: f32, y: f32, z: f32| ColorVertex { pos: [x, y, z], color: c };
    let v = [
        p(0.0, 0.0, 0.0),
        p(1.0, 0.0, 0.0),
        p(1.0, 0.0, 1.0),
        p(0.0, 0.0, 1.0),
        p(0.0, 1.0, 0.0),
        p(1.0, 1.0, 0.0),
        p(1.0, 1.0, 1.0),
        p(0.0, 1.0, 1.0),
    ];
    let i = [0, 1, 1, 2, 2, 3, 3, 0, 4, 5, 5, 6, 6, 7, 7, 4, 0, 4, 1, 5, 2, 6, 3, 7];
    Mesh::new(gl, &v, &i, glow::LINES)
}

impl VoxelRenderer {
    pub fn new(gl: &Gl) -> Result<Self, SpaceError> {
        Ok(Self {
            textures: atlas::create_block_textures(gl)?,
            chunks: HashMap::new(),
            outline: outline_mesh(gl)?,
            uploads: 0,
        })
    }

    /// Replace a chunk's GPU meshes with freshly built ones.
    pub fn upload(&mut self, gl: &Gl, data: ChunkMeshData) -> Result<(), SpaceError> {
        let mk = |m: &crate::world::mesh::MeshData| -> Result<Option<Mesh>, SpaceError> {
            if m.indices.is_empty() {
                Ok(None)
            } else {
                Mesh::new(gl, &m.vertices, &m.indices, glow::TRIANGLES).map(Some)
            }
        };
        let gpu = ChunkGpu { opaque: mk(&data.opaque)?, translucent: mk(&data.translucent)? };
        if let Some(old) = self.chunks.insert(data.pos, gpu) {
            for m in [old.opaque, old.translucent].into_iter().flatten() {
                m.destroy(gl);
            }
        }
        self.uploads += 1;
        Ok(())
    }

    /// Chunks within render distance and inside the view frustum.
    pub fn visible(&self, camera: &Camera, distance: i32, outline: Option<IVec3>) -> VoxelFrame {
        let frustum = Frustum::from_matrix(camera.projection_matrix() * camera.view_matrix());
        let max = (distance * CHUNK) as f32 + CHUNK as f32;
        let mut frame = VoxelFrame { outline, ..Default::default() };
        let mut translucent: Vec<(IVec3, f32)> = Vec::new();
        for (pos, gpu) in &self.chunks {
            let min = (*pos * CHUNK).as_vec3();
            let center = min + Vec3::splat(CHUNK as f32 / 2.0);
            let d = center - camera.position;
            if (d.x * d.x + d.z * d.z).sqrt() > max {
                continue;
            }
            if !frustum.aabb_visible(min, min + Vec3::splat(CHUNK as f32)) {
                continue;
            }
            if gpu.opaque.is_some() {
                frame.opaque.push(*pos);
            }
            if gpu.translucent.is_some() {
                translucent.push((*pos, d.length_squared()));
            }
        }
        translucent.sort_by(|a, b| b.1.total_cmp(&a.1));
        frame.translucent = translucent.into_iter().map(|(p, _)| p).collect();
        frame
    }

    fn bind(
        &self,
        gl: &Gl,
        program: &mut Program,
        view: &Mat4,
        proj: &Mat4,
        camera_pos: Vec3,
        fog: (Vec3, f32, f32),
    ) {
        program.bind(gl);
        program.set_mat4(gl, "u_view", view);
        program.set_mat4(gl, "u_proj", proj);
        program.set_vec3(gl, "u_sun_dir", SUN_DIR.normalize());
        program.set_vec3(gl, "u_camera_pos", camera_pos);
        program.set_vec3(gl, "u_fog_color", fog.0);
        program.set_f32(gl, "u_fog_start", fog.1);
        program.set_f32(gl, "u_fog_end", fog.2);
        program.set_i32(gl, "u_blocks", 0);
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.textures));
        }
    }

    /// Opaque blocks and leaves (alpha-tested), with back faces culled.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_opaque(
        &self,
        gl: &Gl,
        program: &mut Program,
        frame: &VoxelFrame,
        view: &Mat4,
        proj: &Mat4,
        camera_pos: Vec3,
        fog: (Vec3, f32, f32),
    ) {
        self.bind(gl, program, view, proj, camera_pos, fog);
        program.set_f32(gl, "u_alpha_cut", 0.5);
        unsafe {
            gl.enable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.depth_mask(true);
        }
        for pos in &frame.opaque {
            if let Some(mesh) = self.chunks.get(pos).and_then(|c| c.opaque.as_ref()) {
                program.set_vec3(gl, "u_chunk_origin", (*pos * CHUNK).as_vec3());
                mesh.draw(gl);
            }
        }
        unsafe { gl.bind_texture(glow::TEXTURE_2D_ARRAY, None) };
        gl_check(gl, "voxels opaque");
    }

    /// Glass and water: blended, depth tested but not written, both sides visible (you
    /// can look up at the water surface from below).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_translucent(
        &self,
        gl: &Gl,
        program: &mut Program,
        frame: &VoxelFrame,
        view: &Mat4,
        proj: &Mat4,
        camera_pos: Vec3,
        fog: (Vec3, f32, f32),
    ) {
        if frame.translucent.is_empty() {
            return;
        }
        self.bind(gl, program, view, proj, camera_pos, fog);
        program.set_f32(gl, "u_alpha_cut", 0.004);
        unsafe {
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.depth_mask(false);
        }
        for pos in &frame.translucent {
            if let Some(mesh) = self.chunks.get(pos).and_then(|c| c.translucent.as_ref()) {
                program.set_vec3(gl, "u_chunk_origin", (*pos * CHUNK).as_vec3());
                mesh.draw(gl);
            }
        }
        unsafe {
            gl.depth_mask(true);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        }
        gl_check(gl, "voxels translucent");
    }

    /// Thin dark box around the targeted block (uses the plain color program).
    pub fn draw_outline(&self, gl: &Gl, basic: &mut Program, block: IVec3) {
        let grow = 0.004;
        let model = Mat4::from_translation(block.as_vec3() - Vec3::splat(grow))
            * Mat4::from_scale(Vec3::splat(1.0 + 2.0 * grow));
        basic.bind(gl);
        basic.set_mat4(gl, "u_model", &model);
        self.outline.draw(gl);
    }

    pub fn destroy(&mut self, gl: &Gl) {
        for (_, c) in self.chunks.drain() {
            for m in [c.opaque, c.translucent].into_iter().flatten() {
                m.destroy(gl);
            }
        }
        self.outline.destroy(gl);
        unsafe { gl.delete_texture(self.textures) };
    }
}
