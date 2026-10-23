//! A tiny safe-ish layer over raw OpenGL ES.
//!
//! We use `glow` (enabled by smithay's `renderer_glow` feature) rather than smithay's raw
//! `ffi::Gles2` table: glow gives typed handles (a shader is not a texture), Rust strings
//! and slices instead of pointers, and smithay's `GlowRenderer::with_context` hands it to
//! us directly. Every GL call is still `unsafe` — the GPU driver can't check Rust's rules.
//!
//! We target OpenGL ES 3.0 (`#version 300 es`): it runs on Intel, AMD and NVIDIA, inside
//! nested EGL windows, and on the GBM/DRM path, and it has everything we need (VAOs,
//! instancing, multisample renderbuffers, framebuffer blits).

use std::collections::{HashMap, HashSet};

use glam::{Mat4, Vec3, Vec4};
use smithay::reexports::glow::{self, HasContext};

use crate::error::SpaceError;

pub type Gl = glow::Context;

/// In debug builds, check glGetError after important calls and log it with a label.
pub fn gl_check(gl: &Gl, label: &str) {
    if cfg!(debug_assertions) {
        loop {
            // glGetError returns (and clears) one queued error at a time.
            let err = unsafe { gl.get_error() };
            if err == glow::NO_ERROR {
                break;
            }
            let name = match err {
                glow::INVALID_ENUM => "GL_INVALID_ENUM",
                glow::INVALID_VALUE => "GL_INVALID_VALUE",
                glow::INVALID_OPERATION => "GL_INVALID_OPERATION",
                glow::INVALID_FRAMEBUFFER_OPERATION => "GL_INVALID_FRAMEBUFFER_OPERATION",
                glow::OUT_OF_MEMORY => "GL_OUT_OF_MEMORY",
                _ => "unknown GL error",
            };
            tracing::error!(label, code = err, "{name}");
        }
    }
}

/// Mesa logs errors as "0:12(5): error: ..." — pull out line 12 so the message points
/// at the right place in the file.
fn shader_error_line(log: &str) -> Option<u32> {
    let rest = log.trim_start().strip_prefix("0:")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Compile one shader stage. `name` is the file name, used in error messages.
pub fn compile_shader(gl: &Gl, kind: u32, name: &str, src: &str) -> Result<glow::Shader, SpaceError> {
    unsafe {
        // glCreateShader: allocate an empty shader object of this stage.
        let shader = gl.create_shader(kind).map_err(SpaceError::Render)?;
        // glShaderSource + glCompileShader: hand over the GLSL text and compile it.
        gl.shader_source(shader, src);
        gl.compile_shader(shader);
        // GL_COMPILE_STATUS tells whether it worked; the info log says why not.
        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            let line = shader_error_line(&log)
                .map(|l| format!(" line {l}"))
                .unwrap_or_default();
            return Err(SpaceError::Render(format!("{name}{line}: {}", log.trim())));
        }
        Ok(shader)
    }
}

/// A linked GPU program (vertex + fragment shader) with cached uniform locations.
pub struct Program {
    pub id: glow::Program,
    name: String,
    uniforms: HashMap<String, Option<glow::UniformLocation>>,
    warned: HashSet<String>,
}

/// Link a program; the shader objects are deleted afterwards (the program keeps the code).
pub fn link_program(
    gl: &Gl,
    name: &str,
    vs: glow::Shader,
    fs: glow::Shader,
) -> Result<Program, SpaceError> {
    unsafe {
        let program = gl.create_program().map_err(SpaceError::Render)?;
        // glAttachShader + glLinkProgram: connect the stages and resolve their in/out vars.
        gl.attach_shader(program, vs);
        gl.attach_shader(program, fs);
        gl.link_program(program);
        gl.detach_shader(program, vs);
        gl.detach_shader(program, fs);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(SpaceError::Render(format!(
                "{name}: link failed: {}",
                log.trim()
            )));
        }
        Ok(Program {
            id: program,
            name: name.into(),
            uniforms: HashMap::new(),
            warned: HashSet::new(),
        })
    }
}

/// Compile and link a vertex + fragment pair.
pub fn build_program(gl: &Gl, name: &str, vs_src: &str, fs_src: &str) -> Result<Program, SpaceError> {
    let vs = compile_shader(gl, glow::VERTEX_SHADER, &format!("{name}.vert"), vs_src)?;
    let fs = match compile_shader(gl, glow::FRAGMENT_SHADER, &format!("{name}.frag"), fs_src) {
        Ok(fs) => fs,
        Err(e) => {
            unsafe { gl.delete_shader(vs) };
            return Err(e);
        }
    };
    link_program(gl, name, vs, fs)
}

impl Program {
    /// glUseProgram: make this the program that the next draw calls run.
    pub fn bind(&self, gl: &Gl) {
        unsafe { gl.use_program(Some(self.id)) };
    }

    /// Uniform location, cached. A missing uniform (typo, or optimized out because the
    /// shader doesn't use it) is reported once instead of every frame.
    pub fn uniform_location(&mut self, gl: &Gl, name: &str) -> Option<glow::UniformLocation> {
        if let Some(loc) = self.uniforms.get(name) {
            return *loc;
        }
        let loc = unsafe { gl.get_uniform_location(self.id, name) };
        if loc.is_none() && self.warned.insert(name.to_string()) {
            tracing::warn!(program = self.name, uniform = name, "uniform not found");
        }
        self.uniforms.insert(name.to_string(), loc);
        loc
    }

    pub fn set_mat4(&mut self, gl: &Gl, name: &str, m: &Mat4) {
        let loc = self.uniform_location(gl, name);
        // `false`: our matrices are column-major already, no transpose needed.
        unsafe { gl.uniform_matrix_4_f32_slice(loc.as_ref(), false, &m.to_cols_array()) };
    }
    pub fn set_vec3(&mut self, gl: &Gl, name: &str, v: Vec3) {
        let loc = self.uniform_location(gl, name);
        unsafe { gl.uniform_3_f32(loc.as_ref(), v.x, v.y, v.z) };
    }
    #[allow(dead_code)] // first used by the panel shader (Stage 3)
    pub fn set_vec4(&mut self, gl: &Gl, name: &str, v: Vec4) {
        let loc = self.uniform_location(gl, name);
        unsafe { gl.uniform_4_f32(loc.as_ref(), v.x, v.y, v.z, v.w) };
    }
    pub fn set_f32(&mut self, gl: &Gl, name: &str, v: f32) {
        let loc = self.uniform_location(gl, name);
        unsafe { gl.uniform_1_f32(loc.as_ref(), v) };
    }
    #[allow(dead_code)] // first used by the panel shader (Stage 3)
    pub fn set_i32(&mut self, gl: &Gl, name: &str, v: i32) {
        let loc = self.uniform_location(gl, name);
        unsafe { gl.uniform_1_i32(loc.as_ref(), v) };
    }

    pub fn destroy(&self, gl: &Gl) {
        unsafe { gl.delete_program(self.id) };
    }
}

/// One vertex attribute: where it is inside the vertex struct and how to read it.
pub struct Attrib {
    /// `layout(location = N)` in the vertex shader.
    pub location: u32,
    /// Number of components (3 for a vec3).
    pub components: i32,
    /// glow::FLOAT, glow::UNSIGNED_BYTE, ...
    pub ty: u32,
    /// For integer types: map 0..255 to 0.0..1.0.
    pub normalized: bool,
    /// Byte offset inside the vertex.
    pub offset: i32,
}

/// A vertex struct that knows its GL layout.
pub trait Vertex: bytemuck::Pod {
    fn attribs() -> &'static [Attrib];
}

/// Position + color, used by the floor and the axes.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ColorVertex {
    pub pos: [f32; 3],
    pub color: [f32; 3],
}

impl Vertex for ColorVertex {
    fn attribs() -> &'static [Attrib] {
        &[
            Attrib {
                location: 0,
                components: 3,
                ty: glow::FLOAT,
                normalized: false,
                offset: 0,
            },
            Attrib {
                location: 1,
                components: 3,
                ty: glow::FLOAT,
                normalized: false,
                offset: 12,
            },
        ]
    }
}

/// Geometry on the GPU: a VAO remembering the attribute setup, a VBO with the vertices
/// and an EBO with triangle (or line) indices.
///
/// `Drop` can't free it: GL calls need the context, which only exists inside
/// `renderer.with_context`. Call `destroy` explicitly.
pub struct Mesh {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    index_count: i32,
    /// glow::TRIANGLES or glow::LINES.
    mode: u32,
}

impl Mesh {
    pub fn new<V: Vertex>(
        gl: &Gl,
        vertices: &[V],
        indices: &[u32],
        mode: u32,
    ) -> Result<Self, SpaceError> {
        unsafe {
            // A VAO records which buffers feed which attributes, so drawing later is
            // just "bind VAO, draw".
            let vao = gl.create_vertex_array().map_err(SpaceError::Render)?;
            let vbo = gl.create_buffer().map_err(SpaceError::Render)?;
            let ebo = gl.create_buffer().map_err(SpaceError::Render)?;
            gl.bind_vertex_array(Some(vao));

            // Upload the vertex bytes. STATIC_DRAW: written once, drawn many times.
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(vertices),
                glow::STATIC_DRAW,
            );
            // The index buffer binding is stored in the VAO.
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck::cast_slice(indices),
                glow::STATIC_DRAW,
            );

            let stride = std::mem::size_of::<V>() as i32;
            for a in V::attribs() {
                gl.enable_vertex_attrib_array(a.location);
                if a.ty == glow::FLOAT || a.normalized {
                    gl.vertex_attrib_pointer_f32(
                        a.location,
                        a.components,
                        a.ty,
                        a.normalized,
                        stride,
                        a.offset,
                    );
                } else {
                    // Integer attributes arrive in the shader as ints (uvec/ivec).
                    gl.vertex_attrib_pointer_i32(a.location, a.components, a.ty, stride, a.offset);
                }
            }
            // Unbind the VAO first, so the unbinds below aren't recorded into it.
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            gl_check(gl, "Mesh::new");
            Ok(Self {
                vao,
                vbo,
                ebo,
                index_count: indices.len() as i32,
                mode,
            })
        }
    }

    /// Replace the contents (dynamic meshes, e.g. rebuilt chunks).
    #[allow(dead_code)] // first used for chunk meshes (Stage 5)
    pub fn update<V: Vertex>(&mut self, gl: &Gl, vertices: &[V], indices: &[u32]) {
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(vertices),
                glow::DYNAMIC_DRAW,
            );
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck::cast_slice(indices),
                glow::DYNAMIC_DRAW,
            );
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        self.index_count = indices.len() as i32;
    }

    pub fn draw(&self, gl: &Gl) {
        if self.index_count == 0 {
            return;
        }
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_elements(self.mode, self.index_count, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
        }
    }

    pub fn destroy(&self, gl: &Gl) {
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            gl.delete_buffer(self.ebo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mesa_error_lines() {
        assert_eq!(shader_error_line("0:12(5): error: syntax error"), Some(12));
        assert_eq!(shader_error_line("  0:3(1): error"), Some(3));
        assert_eq!(shader_error_line("ERROR: something"), None);
    }

    #[test]
    fn color_vertex_layout_matches_struct() {
        assert_eq!(std::mem::size_of::<ColorVertex>(), 24);
        assert_eq!(ColorVertex::attribs()[1].offset, 12);
    }
}
