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

use glam::{Mat4, Vec3};
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
    gl::{build_program, gl_check, ColorVertex, Gl, Mesh, Program},
};
use crate::error::SpaceError;

/// Sky blue, also the fog color so the horizon fades into the sky.
pub const SKY_COLOR: [f32; 3] = [0.53, 0.75, 0.95];
const FOG_START: f32 = 40.0;
const FOG_END: f32 = 120.0;
/// The floor must reach past FOG_END (half-size 128 m > 120 m), otherwise its edge shows as
/// a hard line before the fog has faded it out.
const FLOOR_TILES: i32 = 256;

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
    floor: Mesh,
    axes: Mesh,
    targets: Option<SceneTargets>,
    /// MSAA sample count (0 = off).
    samples: i32,
    shader_dir: Option<PathBuf>,
}

fn floor_mesh(gl: &Gl) -> Result<Mesh, SpaceError> {
    let half = FLOOR_TILES / 2;
    let light = [0.36, 0.62, 0.30];
    let dark = [0.30, 0.54, 0.25];
    let mut vertices = Vec::with_capacity((FLOOR_TILES * FLOOR_TILES * 4) as usize);
    let mut indices = Vec::with_capacity((FLOOR_TILES * FLOOR_TILES * 6) as usize);
    for z in -half..half {
        for x in -half..half {
            let color = if (x + z).rem_euclid(2) == 0 {
                light
            } else {
                dark
            };
            let base = vertices.len() as u32;
            let (x0, x1, z0, z1) = (x as f32, x as f32 + 1.0, z as f32, z as f32 + 1.0);
            for (px, pz) in [(x0, z0), (x0, z1), (x1, z1), (x1, z0)] {
                vertices.push(ColorVertex {
                    pos: [px, 0.0, pz],
                    color,
                });
            }
            // Counter-clockwise when seen from above, so the top side is the front face.
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    Mesh::new(gl, &vertices, &indices, glow::TRIANGLES)
}

fn axes_mesh(gl: &Gl) -> Result<Mesh, SpaceError> {
    // Slightly above the floor so the lines don't z-fight with it.
    let o = [0.0, 0.01, 0.0];
    let v = |pos, color| ColorVertex { pos, color };
    let vertices = [
        v(o, [1.0, 0.0, 0.0]),
        v([1.0, 0.01, 0.0], [1.0, 0.0, 0.0]),
        v(o, [0.0, 1.0, 0.0]),
        v([0.0, 1.01, 0.0], [0.0, 1.0, 0.0]),
        v(o, [0.0, 0.0, 1.0]),
        v([0.0, 0.01, 1.0], [0.0, 0.0, 1.0]),
    ];
    Mesh::new(gl, &vertices, &[0, 1, 2, 3, 4, 5], glow::LINES)
}

fn load_program(gl: &Gl, src: &ShaderSource, dir: Option<&PathBuf>) -> Result<Program, SpaceError> {
    match dir {
        Some(dir) => {
            let read = |ext: &str| {
                let path = dir.join(format!("{}.{ext}", src.name));
                std::fs::read_to_string(&path)
                    .map_err(|e| SpaceError::Render(format!("{}: {e}", path.display())))
            };
            build_program(gl, src.name, &read("vert")?, &read("frag")?)
        }
        None => build_program(gl, src.name, src.vert, src.frag),
    }
}

impl SceneRenderer {
    pub fn new(renderer: &mut GlowRenderer, shader_dir: Option<PathBuf>) -> Result<Self, SpaceError> {
        let gl_result = renderer.with_context(|gl| -> Result<_, SpaceError> {
            let basic = load_program(gl, &BASIC, shader_dir.as_ref())?;
            let floor = floor_mesh(gl)?;
            let axes = axes_mesh(gl)?;
            // How many samples the driver supports for MSAA renderbuffers.
            let max_samples = unsafe { gl.get_parameter_i32(glow::MAX_SAMPLES) };
            Ok((basic, floor, axes, max_samples.min(4)))
        });
        let (basic, floor, axes, samples) =
            gl_result.map_err(|e| SpaceError::Render(format!("GL context: {e}")))??;
        let samples = if samples >= 2 { samples } else { 0 };
        tracing::info!(msaa = samples, "3D scene renderer ready");
        Ok(Self {
            basic,
            floor,
            axes,
            targets: None,
            samples,
            shader_dir,
        })
    }

    /// Recompile shaders from `--shader-dir`. On error the old program stays in use.
    pub fn reload_shaders(&mut self, renderer: &mut GlowRenderer) {
        let Some(dir) = self.shader_dir.clone() else {
            tracing::info!("shader reload needs --shader-dir");
            return;
        };
        let result = renderer.with_context(|gl| load_program(gl, &BASIC, Some(&dir)));
        match result {
            Ok(Ok(program)) => {
                let old = std::mem::replace(&mut self.basic, program);
                let _ = renderer.with_context(|gl| old.destroy(gl));
                tracing::info!(dir = %dir.display(), "shaders reloaded");
            }
            Ok(Err(err)) => tracing::error!("shader reload failed, keeping old program: {err}"),
            Err(err) => tracing::error!("shader reload: {err}"),
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
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
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
        self.targets = Some(SceneTargets {
            size,
            msaa,
            resolve_fbo,
            resolve_depth,
            buffer,
        });
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
    ) -> Result<(), SpaceError> {
        self.ensure_targets(renderer, size)?;
        let Some(targets) = self.targets.as_mut() else {
            return Ok(());
        };
        let basic = &mut self.basic;
        let (floor, axes) = (&self.floor, &self.axes);
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
                floor.draw(gl);
                axes.draw(gl);

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

    /// The element showing the last rendered scene, covering the output.
    pub fn element(&self) -> Option<TextureRenderElement<GlesTexture>> {
        let t = self.targets.as_ref()?;
        Some(TextureRenderElement::from_texture_render_buffer(
            (0.0, 0.0),
            &t.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        ))
    }

    pub fn destroy(mut self, renderer: &mut GlowRenderer) {
        self.destroy_targets(renderer);
        let _ = renderer.with_context(|gl| {
            self.basic.destroy(gl);
            self.floor.destroy(gl);
            self.axes.destroy(gl);
        });
    }
}

fn check_complete(gl: &Gl, label: &str) -> Result<(), SpaceError> {
    let status = unsafe { gl.check_framebuffer_status(glow::FRAMEBUFFER) };
    if status == glow::FRAMEBUFFER_COMPLETE {
        Ok(())
    } else {
        Err(SpaceError::Render(format!(
            "{label} framebuffer incomplete: 0x{status:x}"
        )))
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
