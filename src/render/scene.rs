//! The 3D scene renderer.
//!
//! Each frame the scene is drawn into OUR OWN offscreen framebuffer (color + depth,
//! optionally multisampled), then handed to smithay as an ordinary texture element that
//! it composites onto the output together with the cursor. Why not draw straight into
//! the output framebuffer?
//! - Depth: a winit/EGL window or a GBM scanout buffer usually has no depth attachment,
//!   and without a depth buffer far objects paint over near ones.
//! - Anti-aliasing: a multisampled renderbuffer gives smooth edges on every backend.
//! - Correct output transforms (rotated monitors, the flipped winit surface) and damage
//!   tracking come for free, because smithay draws the final texture.
//!
//! The cost is one fullscreen copy per frame, which is negligible.
//!
//! Orientation: smithay expects texture row 0 to be the TOP of the image, while GL puts
//! row 0 at the bottom. We render with a vertically flipped projection so the texture
//! comes out top-down. Flipping Y mirrors triangle winding, hence `glFrontFace(CW)`.
//! (Flipping during the MSAA resolve blit instead is not allowed by GLES 3.0.)

use std::{borrow::Borrow, path::PathBuf};

use glam::{Mat4, Vec2, Vec3, Vec4};
use smithay::{
    backend::renderer::{
        element::{
            texture::{TextureRenderBuffer, TextureRenderElement},
            Kind,
        },
        gles::{GlesRenderer, GlesTexture},
        glow::GlowRenderer,
    },
    reexports::glow::{self, HasContext},
    utils::{Physical, Rectangle, Size, Transform},
};

use super::{
    camera::Camera,
    gl::{build_program, gl_check, Attrib, Gl, Mesh, Program, Vertex},
    voxels::{VoxelFrame, VoxelRenderer, UPLOADS_PER_FRAME},
};
use crate::{error::SpaceError, world::mesh::ChunkMeshData};

/// Sky blue, also the fog color so the horizon fades into the sky.
pub const SKY_COLOR: [f32; 3] = [0.53, 0.75, 0.95];
const FOG_START: f32 = 40.0;
const FOG_END: f32 = 120.0;

struct ShaderSource {
    name: &'static str,
    vert: &'static str,
    frag: &'static str,
}

const BASIC: ShaderSource = ShaderSource {
    name: "basic",
    vert: include_str!("shaders/basic.vert"),
    frag: include_str!("shaders/basic.frag"),
};

const VOXEL: ShaderSource = ShaderSource {
    name: "voxel",
    vert: include_str!("shaders/voxel.vert"),
    frag: include_str!("shaders/voxel.frag"),
};

const PANEL: ShaderSource = ShaderSource {
    name: "panel",
    vert: include_str!("shaders/panel.vert"),
    frag: include_str!("shaders/panel.frag"),
};

/// Vertex of the unit panel quad.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PanelVertex {
    pos: [f32; 2],
}

impl Vertex for PanelVertex {
    fn attribs() -> &'static [Attrib] {
        &[Attrib { location: 0, components: 2, ty: glow::FLOAT, normalized: false, offset: 0 }]
    }
}

/// Everything needed to draw one panel this frame.
pub struct PanelDraw {
    pub texture: glow::Texture,
    /// Unit quad → the window bbox in the world.
    pub model: Mat4,
    pub tex_size: Vec2,
    /// Window geometry inside the texture, x0 y0 x1 y1 in px.
    pub geo_rect: Vec4,
    pub opacity: f32,
    pub border_px: f32,
    pub border_color: Vec4,
    /// Opaque panels write depth; translucent ones (drawn last, far to near) don't.
    pub opaque: bool,
}

/// Offscreen targets, recreated when the output size changes.
struct SceneTargets {
    size: Size<i32, Physical>,
    /// Multisampled color + depth we draw into (None when MSAA is off).
    msaa: Option<(glow::Framebuffer, glow::Renderbuffer, glow::Renderbuffer)>,
    /// Single-sampled framebuffer whose color attachment is the texture smithay draws.
    resolve_fbo: glow::Framebuffer,
    /// Depth for `resolve_fbo`, only when drawing into it directly (no MSAA).
    resolve_depth: Option<glow::Renderbuffer>,
    /// The texture + its damage history, shared with smithay's element.
    buffer: TextureRenderBuffer<GlesTexture>,
}

pub struct SceneRenderer {
    basic: Program,
    panel: Program,
    voxel: Program,
    pub voxels: VoxelRenderer,
    quad: Mesh,
    targets: Option<SceneTargets>,
    /// MSAA sample count (0 = off).
    samples: i32,
    shader_dir: Option<PathBuf>,
}

fn quad_mesh(gl: &Gl) -> Result<Mesh, SpaceError> {
    let v = |x, y| PanelVertex { pos: [x, y] };
    // Counter-clockwise seen from +Z: the panel's front face.
    let vertices = [v(-0.5, -0.5), v(0.5, -0.5), v(0.5, 0.5), v(-0.5, 0.5)];
    Mesh::new(gl, &vertices, &[0, 1, 2, 0, 2, 3], glow::TRIANGLES)
}

fn load_program(gl: &Gl, src: &ShaderSource, dir: Option<&PathBuf>) -> Result<Program, SpaceError> {
    let Some(dir) = dir else {
        return build_program(gl, src.name, src.vert, src.frag);
    };
    // A file missing from --shader-dir falls back to the built-in copy, so you can put
    // just the shader you are editing there.
    let read = |ext: &str, builtin: &'static str| -> Result<String, SpaceError> {
        let path = dir.join(format!("{}.{ext}", src.name));
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(path = %path.display(), "not in --shader-dir, using built-in");
                Ok(builtin.to_string())
            }
            Err(e) => Err(SpaceError::Render(format!("{}: {e}", path.display()))),
        }
    };
    build_program(gl, src.name, &read("vert", src.vert)?, &read("frag", src.frag)?)
}

impl SceneRenderer {
    pub fn new(renderer: &mut GlowRenderer, shader_dir: Option<PathBuf>) -> Result<Self, SpaceError> {
        let gl_result = renderer.with_context(|gl| -> Result<_, SpaceError> {
            let basic = load_program(gl, &BASIC, shader_dir.as_ref())?;
            let panel = load_program(gl, &PANEL, shader_dir.as_ref())?;
            let voxel = load_program(gl, &VOXEL, shader_dir.as_ref())?;
            let voxels = VoxelRenderer::new(gl)?;
            let quad = quad_mesh(gl)?;
            // How many samples the driver supports for MSAA renderbuffers.
            let max_samples = unsafe { gl.get_parameter_i32(glow::MAX_SAMPLES) };
            Ok((basic, panel, voxel, voxels, quad, max_samples.min(4)))
        });
        let (basic, panel, voxel, voxels, quad, samples) =
            gl_result.map_err(|e| SpaceError::Render(format!("GL context: {e}")))??;
        let samples = if samples >= 2 { samples } else { 0 };
        tracing::info!(msaa = samples, "3D scene renderer ready");
        Ok(Self { basic, panel, voxel, voxels, quad, targets: None, samples, shader_dir })
    }

    /// Recompile shaders from `--shader-dir`. On error the old program stays in use.
    pub fn reload_shaders(&mut self, renderer: &mut GlowRenderer) {
        let Some(dir) = self.shader_dir.clone() else {
            tracing::info!("shader reload needs --shader-dir");
            return;
        };
        let slots =
            [(&BASIC, &mut self.basic), (&PANEL, &mut self.panel), (&VOXEL, &mut self.voxel)];
        for (src, slot) in slots {
            match renderer.with_context(|gl| load_program(gl, src, Some(&dir))) {
                Ok(Ok(program)) => {
                    let old = std::mem::replace(slot, program);
                    let _ = renderer.with_context(|gl| old.destroy(gl));
                    tracing::info!(dir = %dir.display(), shader = src.name, "shaders reloaded");
                }
                Ok(Err(err)) => {
                    tracing::error!("shader reload failed, keeping old program: {err}")
                }
                Err(err) => tracing::error!("shader reload: {err}"),
            }
        }
    }

    fn ensure_targets(
        &mut self,
        renderer: &mut GlowRenderer,
        size: Size<i32, Physical>,
    ) -> Result<(), SpaceError> {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return Ok(());
        }
        self.destroy_targets(renderer);
        let samples = self.samples;
        let (w, h) = (size.w, size.h);
        let created = renderer.with_context(|gl| unsafe {
            // The texture smithay will composite. RGBA8 with immutable storage, so it matches
            // the MSAA renderbuffer format exactly (required for the resolve blit).
            let tex = gl.create_texture().map_err(SpaceError::Render)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, w, h);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.bind_texture(glow::TEXTURE_2D, None);

            let resolve_fbo = gl.create_framebuffer().map_err(SpaceError::Render)?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(resolve_fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(tex),
                0,
            );
            let resolve_depth = if samples == 0 {
                let rb = gl.create_renderbuffer().map_err(SpaceError::Render)?;
                gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rb));
                gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, w, h);
                gl.framebuffer_renderbuffer(
                    glow::FRAMEBUFFER,
                    glow::DEPTH_ATTACHMENT,
                    glow::RENDERBUFFER,
                    Some(rb),
                );
                Some(rb)
            } else {
                None
            };
            check_complete(gl, "resolve")?;

            let msaa = if samples > 0 {
                let fbo = gl.create_framebuffer().map_err(SpaceError::Render)?;
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                let color = gl.create_renderbuffer().map_err(SpaceError::Render)?;
                gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color));
                gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples, glow::RGBA8, w, h);
                gl.framebuffer_renderbuffer(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::RENDERBUFFER,
                    Some(color),
                );
                let depth = gl.create_renderbuffer().map_err(SpaceError::Render)?;
                gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
                gl.renderbuffer_storage_multisample(
                    glow::RENDERBUFFER,
                    samples,
                    glow::DEPTH_COMPONENT24,
                    w,
                    h,
                );
                gl.framebuffer_renderbuffer(
                    glow::FRAMEBUFFER,
                    glow::DEPTH_ATTACHMENT,
                    glow::RENDERBUFFER,
                    Some(depth),
                );
                check_complete(gl, "msaa")?;
                Some((fbo, color, depth))
            } else {
                None
            };
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl_check(gl, "scene targets");
            Ok::<_, SpaceError>((tex, resolve_fbo, resolve_depth, msaa))
        });
        let (tex, resolve_fbo, resolve_depth, msaa) =
            created.map_err(|e| SpaceError::Render(format!("GL context: {e}")))??;

        // Hand the texture to smithay. It takes ownership and deletes it when dropped.
        let gles: &GlesRenderer = (*renderer).borrow();
        // SAFETY: `tex` is a valid, complete RGBA8 texture created on this renderer's context.
        let texture = unsafe {
            GlesTexture::from_raw(gles, Some(glow::RGBA8), true, tex.0.get(), (w, h).into())
        };
        let buffer =
            TextureRenderBuffer::from_texture(renderer, texture, 1, Transform::Normal, None);
        self.targets = Some(SceneTargets { size, msaa, resolve_fbo, resolve_depth, buffer });
        tracing::debug!(w, h, samples, "scene targets (re)created");
        Ok(())
    }

    fn destroy_targets(&mut self, renderer: &mut GlowRenderer) {
        if let Some(t) = self.targets.take() {
            let _ = renderer.with_context(|gl| unsafe {
                gl.delete_framebuffer(t.resolve_fbo);
                if let Some(rb) = t.resolve_depth {
                    gl.delete_renderbuffer(rb);
                }
                if let Some((fbo, color, depth)) = t.msaa {
                    gl.delete_framebuffer(fbo);
                    gl.delete_renderbuffer(color);
                    gl.delete_renderbuffer(depth);
                }
            });
            // The texture itself is freed when `t.buffer` drops.
        }
    }

    /// Draw the scene into the offscreen texture.
    pub fn render(
        &mut self,
        renderer: &mut GlowRenderer,
        camera: &Camera,
        size: Size<i32, Physical>,
        panels: &[PanelDraw],
        voxel_frame: &VoxelFrame,
    ) -> Result<(), SpaceError> {
        self.ensure_targets(renderer, size)?;
        let Some(targets) = self.targets.as_mut() else {
            return Ok(());
        };
        let basic = &mut self.basic;
        let panel_program = &mut self.panel;
        let voxel_program = &mut self.voxel;
        let voxels = &self.voxels;
        let quad = &self.quad;
        let fog = (Vec3::from(SKY_COLOR), FOG_START, FOG_END);
        let (w, h) = (size.w, size.h);
        // Flip Y (see module docs) so the texture comes out top-row-first.
        let proj = Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0)) * camera.projection_matrix();
        let view = camera.view_matrix();
        let draw_fbo = targets.msaa.map(|m| m.0).unwrap_or(targets.resolve_fbo);
        let msaa = targets.msaa;
        let resolve_fbo = targets.resolve_fbo;

        renderer
            .with_context(|gl| unsafe {
                // Draw into our framebuffer, covering all of it.
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(draw_fbo));
                gl.viewport(0, 0, w, h);
                gl.disable(glow::SCISSOR_TEST);
                gl.disable(glow::BLEND);
                // Depth test: keep a pixel only if it is nearer than what's already there.
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LESS);
                gl.depth_mask(true);
                // Skip triangles facing away from the camera (half the work for closed shapes).
                gl.enable(glow::CULL_FACE);
                gl.cull_face(glow::BACK);
                gl.front_face(glow::CW);

                gl.clear_color(SKY_COLOR[0], SKY_COLOR[1], SKY_COLOR[2], 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

                basic.bind(gl);
                basic.set_mat4(gl, "u_view", &view);
                basic.set_mat4(gl, "u_proj", &proj);
                basic.set_mat4(gl, "u_model", &Mat4::IDENTITY);
                basic.set_vec3(gl, "u_camera_pos", camera.position);
                basic.set_vec3(gl, "u_fog_color", Vec3::from(SKY_COLOR));
                basic.set_f32(gl, "u_fog_start", FOG_START);
                basic.set_f32(gl, "u_fog_end", FOG_END);
                // Draw order matters for transparency:
                // 1. opaque blocks (and alpha-tested leaves), 2. the aimed-at block's outline,
                // 3. opaque panels, 4. glass/water far to near, 5. translucent panels and the
                // cursor — each blended layer over everything behind it.
                voxels.draw_opaque(
                    gl,
                    voxel_program,
                    voxel_frame,
                    &view,
                    &proj,
                    camera.position,
                    fog,
                );
                if let Some(block) = voxel_frame.outline {
                    voxels.draw_outline(gl, basic, block);
                }
                draw_panels(gl, panel_program, quad, panels, true, &view, &proj, camera.position);
                voxels.draw_translucent(
                    gl,
                    voxel_program,
                    voxel_frame,
                    &view,
                    &proj,
                    camera.position,
                    fog,
                );
                draw_panels(gl, panel_program, quad, panels, false, &view, &proj, camera.position);

                if let Some((msaa_fbo, _, _)) = msaa {
                    // Resolve: average the samples of each pixel into the texture.
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(msaa_fbo));
                    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(resolve_fbo));
                    gl.blit_framebuffer(
                        0,
                        0,
                        w,
                        h,
                        0,
                        0,
                        w,
                        h,
                        glow::COLOR_BUFFER_BIT,
                        glow::NEAREST,
                    );
                    // Tell the driver the MSAA contents are no longer needed (saves memory
                    // bandwidth on tiled GPUs).
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(msaa_fbo));
                    gl.invalidate_framebuffer(
                        glow::FRAMEBUFFER,
                        &[glow::COLOR_ATTACHMENT0, glow::DEPTH_ATTACHMENT],
                    );
                }
                gl_check(gl, "scene render");
                restore_smithay_state(gl);
            })
            .map_err(|e| SpaceError::Render(format!("GL context: {e}")))?;

        // Record that the whole texture changed, so smithay redraws it.
        let full = Rectangle::from_size((w, h).into());
        let _ = targets.buffer.render().draw(|_| Ok::<_, ()>(vec![full]));
        Ok(())
    }

    /// Upload up to `UPLOADS_PER_FRAME` finished chunk meshes. Returns how many.
    pub fn upload_chunks(
        &mut self,
        renderer: &mut GlowRenderer,
        pending: &mut Vec<ChunkMeshData>,
    ) -> usize {
        let n = pending.len().min(UPLOADS_PER_FRAME);
        if n == 0 {
            return 0;
        }
        let batch: Vec<_> = pending.drain(..n).collect();
        let voxels = &mut self.voxels;
        let _ = renderer.with_context(|gl| {
            for data in batch {
                if let Err(err) = voxels.upload(gl, data) {
                    tracing::warn!("chunk upload failed: {err}");
                }
            }
            restore_smithay_state(gl);
        });
        n
    }

    /// The element showing the last rendered scene, covering the output.
    pub fn element(&self) -> Option<TextureRenderElement<GlesTexture>> {
        self.element_sized(None)
    }

    /// The scene element, optionally stretched to `size` (logical px) — for outputs whose
    /// size differs from the texture the scene was rendered into.
    pub fn element_sized(
        &self,
        size: Option<Size<i32, smithay::utils::Logical>>,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        let t = self.targets.as_ref()?;
        Some(TextureRenderElement::from_texture_render_buffer(
            (0.0, 0.0),
            &t.buffer,
            None,
            None,
            size,
            Kind::Unspecified,
        ))
    }

    pub fn destroy(mut self, renderer: &mut GlowRenderer) {
        self.destroy_targets(renderer);
        let _ = renderer.with_context(|gl| {
            self.basic.destroy(gl);
            self.panel.destroy(gl);
            self.voxel.destroy(gl);
            self.voxels.destroy(gl);
            self.quad.destroy(gl);
        });
    }
}

/// Panels are flat and visible from both sides: no face culling (the shader darkens
/// the back). Premultiplied alpha blending: out = src + dst·(1 − src.a).
#[allow(clippy::too_many_arguments)]
fn draw_panels(
    gl: &Gl,
    program: &mut Program,
    quad: &Mesh,
    panels: &[PanelDraw],
    opaque_pass: bool,
    view: &Mat4,
    proj: &Mat4,
    camera_pos: Vec3,
) {
    if !panels.iter().any(|p| p.opaque == opaque_pass) {
        return;
    }
    unsafe {
        gl.disable(glow::CULL_FACE);
        gl.enable(glow::BLEND);
        gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        gl.active_texture(glow::TEXTURE0);
    }
    program.bind(gl);
    program.set_mat4(gl, "u_view", view);
    program.set_mat4(gl, "u_proj", proj);
    program.set_vec3(gl, "u_camera_pos", camera_pos);
    program.set_vec3(gl, "u_fog_color", Vec3::from(SKY_COLOR));
    program.set_f32(gl, "u_fog_start", FOG_START);
    program.set_f32(gl, "u_fog_end", FOG_END);
    program.set_i32(gl, "u_tex", 0);
    for p in panels.iter().filter(|p| p.opaque == opaque_pass) {
        unsafe {
            gl.depth_mask(p.opaque);
            gl.bind_texture(glow::TEXTURE_2D, Some(p.texture));
        }
        program.set_mat4(gl, "u_model", &p.model);
        program.set_f32(gl, "u_opacity", p.opacity);
        unsafe {
            let loc = program.uniform_location(gl, "u_tex_size");
            gl.uniform_2_f32(loc.as_ref(), p.tex_size.x, p.tex_size.y);
        }
        program.set_vec4(gl, "u_geo_rect", p.geo_rect);
        program.set_f32(gl, "u_border_px", p.border_px);
        program.set_vec4(gl, "u_border_color", p.border_color);
        quad.draw(gl);
    }
    unsafe {
        gl.depth_mask(true);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
    gl_check(gl, "panels");
}

fn check_complete(gl: &Gl, label: &str) -> Result<(), SpaceError> {
    let status = unsafe { gl.check_framebuffer_status(glow::FRAMEBUFFER) };
    if status == glow::FRAMEBUFFER_COMPLETE {
        Ok(())
    } else {
        Err(SpaceError::Render(format!("{label} framebuffer incomplete: 0x{status:x}")))
    }
}

/// smithay's renderer assumes GL is in its default state. Leave it the way we found it:
/// no depth test or culling, default winding, blending on, nothing of ours bound.
fn restore_smithay_state(gl: &Gl) {
    unsafe {
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.front_face(glow::CCW);
        gl.depth_mask(true);
        gl.enable(glow::BLEND);
        gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        gl.use_program(None);
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    }
}
