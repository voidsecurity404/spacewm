//! Drawing the map (world/map.rs) on the GPU: one mesh, one draw call.
//!
//! Textures: a texture array (one layer per material, 512x512, with mipmaps and
//! anisotropic filtering, so floors stay sharp at grazing angles) for the colors and
//! one for the normal maps (the bumps of stone and wood, lit per pixel).
//!
//! Shadows: a shadow map, a depth picture of the map seen from the sun. A pixel whose
//! distance to the sun is larger than the depth stored there is behind something: in
//! shadow. The map never moves, so it is drawn once, and again only when the sun
//! moves (Settings: time of day): shadows cost almost nothing per frame.

use glam::{Mat4, Vec3};
use smithay::reexports::glow::{self, HasContext};

use super::gl::{gl_check, Attrib, Gl, Mesh, Program, Vertex};
use crate::{
    error::SpaceError,
    world::map::{Aabb, Map, MapVertex, TEXTURES},
};

impl Vertex for MapVertex {
    fn attribs() -> &'static [Attrib] {
        const F: u32 = glow::FLOAT;
        // Offsets in bytes of the fields of MapVertex (#[repr(C)], all f32).
        const A: [Attrib; 7] = [
            Attrib { location: 0, components: 3, ty: F, normalized: false, offset: 0 },
            Attrib { location: 1, components: 3, ty: F, normalized: false, offset: 12 },
            Attrib { location: 2, components: 3, ty: F, normalized: false, offset: 24 },
            Attrib { location: 3, components: 2, ty: F, normalized: false, offset: 36 },
            Attrib { location: 4, components: 1, ty: F, normalized: false, offset: 44 },
            Attrib { location: 5, components: 3, ty: F, normalized: false, offset: 48 },
            Attrib { location: 6, components: 1, ty: F, normalized: false, offset: 60 },
        ];
        &A
    }
}

/// Texture size of every material layer.
const TEX: i32 = 512;
/// Shadow map resolution: about 5 cm per texel over the ~100 m map.
const SHADOW_SIZE: i32 = 2048;

/// Light and sky for one time of day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapLook {
    /// Toward the sun.
    pub sun_dir: Vec3,
    pub sun_color: Vec3,
    /// The sky at the horizon (also the fog) and straight up.
    pub sky: Vec3,
    pub zenith: Vec3,
    pub sky_ambient: Vec3,
    pub ground_ambient: Vec3,
    pub shadows: bool,
}

impl MapLook {
    /// "day", "afternoon" or "evening". Shadows are light: in a desert the sand throws
    /// a lot of sunlight back up (the warm ground ambient).
    pub fn for_time(time: &str, shadows: bool) -> Self {
        match time {
            "evening" => Self {
                sun_dir: Vec3::new(-0.75, 0.26, 0.45).normalize(),
                sun_color: Vec3::new(1.50, 0.90, 0.55),
                sky: Vec3::new(0.98, 0.72, 0.50),
                zenith: Vec3::new(0.38, 0.47, 0.68),
                sky_ambient: Vec3::new(0.52, 0.50, 0.60),
                ground_ambient: Vec3::new(0.50, 0.36, 0.26),
                shadows,
            },
            "afternoon" => Self {
                sun_dir: Vec3::new(-0.55, 0.62, 0.35).normalize(),
                sun_color: Vec3::new(1.40, 1.18, 0.90),
                sky: Vec3::new(0.84, 0.82, 0.78),
                zenith: Vec3::new(0.36, 0.55, 0.82),
                sky_ambient: Vec3::new(0.56, 0.58, 0.66),
                ground_ambient: Vec3::new(0.58, 0.46, 0.32),
                shadows,
            },
            _ => Self {
                sun_dir: Vec3::new(0.45, 0.80, 0.32).normalize(),
                sun_color: Vec3::new(1.32, 1.22, 1.04),
                sky: Vec3::new(0.80, 0.84, 0.88),
                zenith: Vec3::new(0.30, 0.52, 0.86),
                sky_ambient: Vec3::new(0.56, 0.62, 0.72),
                ground_ambient: Vec3::new(0.62, 0.50, 0.36),
                shadows,
            },
        }
    }
}

struct ShadowMap {
    fbo: glow::Framebuffer,
    depth: glow::Texture,
    /// World -> texture coordinates and depth (0..1).
    matrix: Mat4,
    /// The sun it was drawn for.
    sun: Vec3,
}

pub struct MapRenderer {
    mesh: Mesh,
    /// An empty vertex array for the sky triangle (its corners come from the vertex
    /// number; GLES still wants a vertex array bound).
    sky_vao: glow::VertexArray,
    albedo: glow::Texture,
    normals: glow::Texture,
    shadow: Option<ShadowMap>,
    bounds: Aabb,
    /// Which map this is (a new map replaces the renderer).
    pub generation: u64,
    bytes: usize,
}

/// Decode a JPEG into RGBA, `TEX` x `TEX`.
fn decode(name: &str, data: &[u8]) -> Result<Vec<u8>, SpaceError> {
    let err = |e: String| SpaceError::Render(format!("texture {name}: {e}"));
    let mut decoder = zune_jpeg::JpegDecoder::new(data);
    let rgb = decoder.decode().map_err(|e| err(e.to_string()))?;
    let (w, h) = decoder.dimensions().ok_or_else(|| err("no size".into()))?;
    if (w as i32, h as i32) != (TEX, TEX) || rgb.len() != w * h * 3 {
        return Err(err(format!("must be {TEX}x{TEX} RGB, is {w}x{h}")));
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for px in rgb.as_chunks::<3>().0 {
        rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    Ok(rgba)
}

/// The texture files, built into spacewm (so it never misses them).
fn texture_bytes(name: &str, normal: bool) -> &'static [u8] {
    macro_rules! tex {
        ($n:literal) => {
            if normal {
                &include_bytes!(concat!("../../resources/maps/dust/textures/", $n, "_n.jpg"))[..]
            } else {
                &include_bytes!(concat!("../../resources/maps/dust/textures/", $n, ".jpg"))[..]
            }
        };
    }
    match name {
        "sandstone" => tex!("sandstone"),
        "plaster" => tex!("plaster"),
        "trim" => tex!("trim"),
        "sand" => tex!("sand"),
        "paving" => tex!("paving"),
        "crate" => tex!("crate"),
        "door" => tex!("door"),
        _ => tex!("metal"),
    }
}

fn texture_array(gl: &Gl, normal: bool) -> Result<glow::Texture, SpaceError> {
    // 512 -> 256 -> ... -> 1: ten mip levels.
    let levels = 10;
    let layers = TEXTURES.len() as i32;
    unsafe {
        let tex = gl.create_texture().map_err(SpaceError::Render)?;
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(tex));
        gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY, levels, glow::RGBA8, TEX, TEX, layers);
        for (layer, name) in TEXTURES.iter().enumerate() {
            let pixels = decode(name, texture_bytes(name, normal))?;
            gl.tex_sub_image_3d(
                glow::TEXTURE_2D_ARRAY,
                0,
                0,
                0,
                layer as i32,
                TEX,
                TEX,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&pixels)),
            );
        }
        gl.generate_mipmap(glow::TEXTURE_2D_ARRAY);
        let p = |k, v: u32| gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, k, v as i32);
        p(glow::TEXTURE_MAG_FILTER, glow::LINEAR);
        p(glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR);
        p(glow::TEXTURE_WRAP_S, glow::REPEAT);
        p(glow::TEXTURE_WRAP_T, glow::REPEAT);
        // Anisotropic filtering (an extension, everywhere in practice): floors seen at
        // a low angle stay sharp instead of turning into a blur.
        if gl.supported_extensions().contains("GL_EXT_texture_filter_anisotropic") {
            gl.tex_parameter_f32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_MAX_ANISOTROPY_EXT, 8.0);
        }
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        Ok(tex)
    }
}

impl MapRenderer {
    pub fn new(gl: &Gl, map: &Map, generation: u64) -> Result<Self, SpaceError> {
        let mesh = Mesh::new(gl, &map.mesh.vertices, &map.mesh.indices, glow::TRIANGLES)?;
        let sky_vao = unsafe { gl.create_vertex_array().map_err(SpaceError::Render)? };
        let albedo = texture_array(gl, false)?;
        let normals = texture_array(gl, true)?;
        let bytes = std::mem::size_of_val(&map.mesh.vertices[..])
            + std::mem::size_of_val(&map.mesh.indices[..])
            + 2 * (TEX * TEX * 4) as usize * TEXTURES.len() * 4 / 3;
        gl_check(gl, "map upload");
        tracing::info!(
            triangles = map.mesh.indices.len() / 3,
            textures = TEXTURES.len(),
            "map uploaded"
        );
        Ok(Self {
            mesh,
            sky_vao,
            albedo,
            normals,
            shadow: None,
            bounds: map.bounds,
            generation,
            bytes,
        })
    }

    pub fn gpu_bytes(&self) -> usize {
        self.bytes + self.shadow.as_ref().map_or(0, |_| (SHADOW_SIZE * SHADOW_SIZE * 4) as usize)
    }

    /// The sun's camera: looking along the light at the whole map (orthographic: the
    /// sun is so far that its rays are parallel).
    fn sun_view_proj(&self, sun: Vec3) -> Mat4 {
        let center = (self.bounds.min + self.bounds.max) * 0.5;
        let radius = (self.bounds.max - self.bounds.min).length() * 0.5;
        let eye = center + sun * radius * 2.0;
        let up = if sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        let view = Mat4::look_at_rh(eye, center, up);
        let proj = Mat4::orthographic_rh_gl(-radius, radius, -radius, radius, 0.1, radius * 4.0);
        proj * view
    }

    /// Draw the shadow map if the sun moved since (or it was never drawn). Leaves the
    /// framebuffer unbound: the caller binds its own next.
    pub fn update_shadow(&mut self, gl: &Gl, program: &mut Program, look: &MapLook) {
        if !look.shadows || self.shadow.as_ref().is_some_and(|s| s.sun == look.sun_dir) {
            return;
        }
        let view_proj = self.sun_view_proj(look.sun_dir);
        unsafe {
            if self.shadow.is_none() {
                let Ok(depth) = gl.create_texture() else { return };
                gl.bind_texture(glow::TEXTURE_2D, Some(depth));
                gl.tex_storage_2d(
                    glow::TEXTURE_2D,
                    1,
                    glow::DEPTH_COMPONENT24,
                    SHADOW_SIZE,
                    SHADOW_SIZE,
                );
                let p = |k, v: u32| gl.tex_parameter_i32(glow::TEXTURE_2D, k, v as i32);
                // LINEAR + compare mode: the hardware compares 4 texels and blends them.
                p(glow::TEXTURE_MIN_FILTER, glow::LINEAR);
                p(glow::TEXTURE_MAG_FILTER, glow::LINEAR);
                p(glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE);
                p(glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE);
                p(glow::TEXTURE_COMPARE_MODE, glow::COMPARE_REF_TO_TEXTURE);
                p(glow::TEXTURE_COMPARE_FUNC, glow::LEQUAL);
                gl.bind_texture(glow::TEXTURE_2D, None);
                let Ok(fbo) = gl.create_framebuffer() else {
                    gl.delete_texture(depth);
                    return;
                };
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::DEPTH_ATTACHMENT,
                    glow::TEXTURE_2D,
                    Some(depth),
                    0,
                );
                gl.draw_buffers(&[glow::NONE]);
                gl.read_buffer(glow::NONE);
                if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                    tracing::warn!("shadow framebuffer incomplete: no shadows");
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    gl.delete_framebuffer(fbo);
                    gl.delete_texture(depth);
                    return;
                }
                self.shadow =
                    Some(ShadowMap { fbo, depth, matrix: Mat4::IDENTITY, sun: Vec3::ZERO });
            }
            let Some(shadow) = self.shadow.as_mut() else { return };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(shadow.fbo));
            gl.viewport(0, 0, SHADOW_SIZE, SHADOW_SIZE);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.depth_mask(true);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            // Only the back sides (as seen from the sun) go in: a lit face then never
            // shadows itself, the classic cure for "shadow acne".
            gl.enable(glow::CULL_FACE);
            gl.front_face(glow::CCW);
            gl.cull_face(glow::FRONT);
            program.bind(gl);
            program.set_mat4(gl, "u_sun_view_proj", &view_proj);
            self.mesh.draw(gl);
            gl.cull_face(glow::BACK);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            // NDC (-1..1) -> texture space (0..1) for x, y and depth.
            let bias =
                Mat4::from_translation(Vec3::splat(0.5)) * Mat4::from_scale(Vec3::splat(0.5));
            shadow.matrix = bias * view_proj;
            shadow.sun = look.sun_dir;
        }
        gl_check(gl, "map shadow");
        tracing::debug!(sun = ?look.sun_dir, "shadow map drawn");
    }

    /// The sky behind everything: drawn first, writes no depth.
    pub fn draw_sky(
        &self,
        gl: &Gl,
        program: &mut Program,
        view_proj: &Mat4,
        camera_pos: Vec3,
        look: &MapLook,
    ) {
        program.bind(gl);
        program.set_mat4(gl, "u_inv_view_proj", &view_proj.inverse());
        program.set_vec3(gl, "u_camera_pos", camera_pos);
        program.set_vec3(gl, "u_horizon", look.sky);
        program.set_vec3(gl, "u_zenith", look.zenith);
        program.set_vec3(gl, "u_sun_dir", look.sun_dir);
        program.set_vec3(gl, "u_sun_color", look.sun_color);
        unsafe {
            gl.disable(glow::DEPTH_TEST);
            gl.depth_mask(false);
            gl.disable(glow::CULL_FACE);
            gl.bind_vertex_array(Some(self.sky_vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            gl.depth_mask(true);
            gl.enable(glow::DEPTH_TEST);
            gl.enable(glow::CULL_FACE);
        }
        gl_check(gl, "sky");
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        gl: &Gl,
        program: &mut Program,
        view: &Mat4,
        proj: &Mat4,
        camera_pos: Vec3,
        fog: (Vec3, f32, f32),
        look: &MapLook,
    ) {
        program.bind(gl);
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
        program.set_i32(gl, "u_albedo", 0);
        program.set_i32(gl, "u_normals", 1);
        program.set_i32(gl, "u_shadow", 2);
        program.set_f32(gl, "u_shadow_texel", 0.75 / SHADOW_SIZE as f32);
        let shadow = self.shadow.as_ref().filter(|s| look.shadows && s.sun == look.sun_dir);
        program.set_i32(gl, "u_shadows", i32::from(shadow.is_some()));
        program.set_mat4(gl, "u_shadow_matrix", &shadow.map_or(Mat4::IDENTITY, |s| s.matrix));
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.albedo));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.normals));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, shadow.map(|s| s.depth));
            gl.enable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.depth_mask(true);
        }
        self.mesh.draw(gl);
        unsafe {
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        }
        gl_check(gl, "map draw");
    }

    pub fn destroy(&mut self, gl: &Gl) {
        self.mesh.destroy(gl);
        unsafe {
            gl.delete_vertex_array(self.sky_vao);
            gl.delete_texture(self.albedo);
            gl.delete_texture(self.normals);
            if let Some(s) = self.shadow.take() {
                gl.delete_framebuffer(s.fbo);
                gl.delete_texture(s.depth);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_decode() {
        for name in TEXTURES {
            for normal in [false, true] {
                let px = decode(name, texture_bytes(name, normal)).expect(name);
                assert_eq!(px.len(), (TEX * TEX * 4) as usize);
            }
        }
    }

    #[test]
    fn vertex_layout_matches_the_struct() {
        assert_eq!(std::mem::size_of::<MapVertex>(), 64);
        let last = MapVertex::attribs().last().expect("attribs");
        assert_eq!(last.offset, 60);
    }

    #[test]
    fn looks() {
        let day = MapLook::for_time("day", true);
        let evening = MapLook::for_time("evening", true);
        assert!(evening.sun_dir.y < day.sun_dir.y, "evening sun is lower");
        assert!((day.sun_dir.length() - 1.0).abs() < 1e-5);
    }
}
