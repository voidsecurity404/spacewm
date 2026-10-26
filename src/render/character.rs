//! Drawing the player's body (world/character.rs): one skinned mesh, one texture, one
//! draw call; the bone matrices go to the vertex shader each frame.

use glam::{Mat4, Vec3};
use smithay::reexports::glow::{self, HasContext};

use super::{
    gl::{gl_check, Attrib, Gl, Mesh, Program, Vertex},
    map::MapLook,
};
use crate::{
    error::SpaceError,
    world::character::{CharacterModel, SkinVertex},
};

impl Vertex for SkinVertex {
    fn attribs() -> &'static [Attrib] {
        const F: u32 = glow::FLOAT;
        // Byte offsets of the fields of SkinVertex (#[repr(C)], all f32).
        const A: [Attrib; 5] = [
            Attrib { location: 0, components: 3, ty: F, normalized: false, offset: 0 },
            Attrib { location: 1, components: 3, ty: F, normalized: false, offset: 12 },
            Attrib { location: 2, components: 2, ty: F, normalized: false, offset: 24 },
            Attrib { location: 3, components: 4, ty: F, normalized: false, offset: 32 },
            Attrib { location: 4, components: 4, ty: F, normalized: false, offset: 48 },
        ];
        &A
    }
}

/// What to draw this frame.
pub struct CharacterDraw {
    /// Joint matrices (world/character.rs `pose`).
    pub joints: Vec<Mat4>,
    /// Model -> world: where the body stands and which way it faces.
    pub model: Mat4,
}

pub struct CharacterRenderer {
    mesh: Mesh,
    texture: glow::Texture,
}

impl CharacterRenderer {
    pub fn new(gl: &Gl, model: &CharacterModel) -> Result<Self, SpaceError> {
        let mesh = Mesh::new(gl, &model.vertices, &model.indices, glow::TRIANGLES)?;
        let (w, h, rgba) = &model.texture;
        let texture = unsafe {
            let tex = gl.create_texture().map_err(SpaceError::Render)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                *w as i32,
                *h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(rgba)),
            );
            // A small palette texture: sharp texels, no blur between color swatches.
            let p = |k, v: u32| gl.tex_parameter_i32(glow::TEXTURE_2D, k, v as i32);
            p(glow::TEXTURE_MIN_FILTER, glow::NEAREST);
            p(glow::TEXTURE_MAG_FILTER, glow::NEAREST);
            p(glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE);
            p(glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE);
            gl.bind_texture(glow::TEXTURE_2D, None);
            tex
        };
        gl_check(gl, "character upload");
        tracing::info!(triangles = model.indices.len() / 3, "character uploaded");
        Ok(Self { mesh, texture })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        gl: &Gl,
        program: &mut Program,
        draw: &CharacterDraw,
        view: &Mat4,
        proj: &Mat4,
        camera_pos: Vec3,
        fog: (Vec3, f32, f32),
        look: &MapLook,
    ) {
        program.bind(gl);
        program.set_mat4(gl, "u_model", &draw.model);
        program.set_mat4(gl, "u_view", view);
        program.set_mat4(gl, "u_proj", proj);
        program.set_vec3(gl, "u_sun_dir", look.sun_dir);
        program.set_vec3(gl, "u_sun_color", look.sun_color);
        program.set_vec3(gl, "u_sky_ambient", look.sky_ambient);
        program.set_vec3(gl, "u_ground_ambient", look.ground_ambient);
        program.set_vec3(gl, "u_camera_pos", camera_pos);
        program.set_vec3(gl, "u_fog_color", fog.0);
        program.set_f32(gl, "u_fog_start", fog.1);
        program.set_f32(gl, "u_fog_end", fog.2);
        program.set_i32(gl, "u_texture", 0);
        let floats: Vec<f32> =
            draw.joints.iter().take(64).flat_map(|m| m.to_cols_array()).collect();
        if let Some(loc) = program.uniform_location(gl, "u_joints[0]") {
            unsafe { gl.uniform_matrix_4_f32_slice(Some(&loc), false, &floats) };
        }
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            // The model's triangles wind the glTF way (counter-clockwise front), like ours.
            gl.enable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.depth_mask(true);
        }
        self.mesh.draw(gl);
        unsafe { gl.bind_texture(glow::TEXTURE_2D, None) };
        gl_check(gl, "character draw");
    }

    pub fn destroy(&mut self, gl: &Gl) {
        self.mesh.destroy(gl);
        unsafe { gl.delete_texture(self.texture) };
    }
}
