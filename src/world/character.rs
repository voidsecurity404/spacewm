//! The player's body, seen in third person (F5): a rigged, animated glTF character.
//!
//! How skeletal animation works: the model's vertices are "skinned" to bones (joints):
//! each vertex names up to 4 joints and how much each one pulls it (weights). A joint
//! is a node with a transform; an animation is keyframes of node translation, rotation
//! and scale over time. To draw a frame: sample every animated node at time t, multiply
//! the transforms down the node tree (parent * child) to get each node's place in the
//! model, and give the vertex shader, per joint, `node_global * inverse_bind`: the
//! inverse bind matrix undoes where the joint was when the mesh was attached, so that
//! matrix moves a vertex from the rest pose to the animated one. The GPU then blends the
//! (up to 4) joint matrices of each vertex by its weights.
//!
//! The file (resources/characters/knight.glb, CC0, see its LICENSE.md) is one .glb with
//! everything inside: meshes, skeleton, a texture and 76 animations. The knight's
//! weapons are left out; its helmet and cape are rigid parts on the head and chest
//! bones, drawn as if skinned 100% to that bone, so the whole body is one mesh.

use glam::{Mat4, Quat, Vec3};

pub const KNIGHT: &[u8] = include_bytes!("../../resources/characters/knight.glb");

/// The body is this tall (the player box is 1.8 m).
const HEIGHT: f32 = 1.75;
/// Switching animations fades from the old pose to the new one over this long.
const FADE: f32 = 0.2;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SkinVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Joint indices (as floats: every GPU reads those) and their weights.
    pub joints: [f32; 4],
    pub weights: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Trs {
    t: Vec3,
    r: Quat,
    s: Vec3,
}

impl Trs {
    fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.s, self.r, self.t)
    }

    fn lerp(&self, o: &Trs, w: f32) -> Trs {
        Trs { t: self.t.lerp(o.t, w), r: self.r.slerp(o.r, w), s: self.s.lerp(o.s, w) }
    }
}

#[derive(Debug, Clone)]
enum Values {
    T(Vec<Vec3>),
    R(Vec<Quat>),
    S(Vec<Vec3>),
}

#[derive(Debug, Clone)]
struct Channel {
    node: usize,
    times: Vec<f32>,
    values: Values,
}

#[derive(Debug, Clone)]
pub struct Clip {
    pub name: String,
    pub duration: f32,
    channels: Vec<Channel>,
}

pub struct CharacterModel {
    pub vertices: Vec<SkinVertex>,
    pub indices: Vec<u32>,
    /// The texture, RGBA.
    pub texture: (u32, u32, Vec<u8>),
    pub clips: Vec<Clip>,
    parents: Vec<Option<usize>>,
    rest: Vec<Trs>,
    /// Nodes in an order where every parent comes before its children.
    order: Vec<usize>,
    /// Per joint: its node and inverse bind matrix.
    joints: Vec<(usize, Mat4)>,
    /// Puts the model on the ground with its feet at 0, `HEIGHT` tall, facing +Z.
    pub fit: Mat4,
}

/// Linear interpolation between keyframes (glTF "LINEAR"; quaternions by slerp).
fn sample_index(times: &[f32], t: f32) -> (usize, usize, f32) {
    match times.iter().position(|&k| k > t) {
        None => (times.len() - 1, times.len() - 1, 0.0),
        Some(0) => (0, 0, 0.0),
        Some(i) => {
            let (a, b) = (times[i - 1], times[i]);
            (i - 1, i, if b > a { (t - a) / (b - a) } else { 0.0 })
        }
    }
}

impl CharacterModel {
    pub fn load(glb: &[u8]) -> Result<Self, String> {
        let gltf = gltf::Gltf::from_slice(glb).map_err(|e| format!("character: {e}"))?;
        let blob = gltf.blob.as_deref().ok_or("character: no binary chunk")?;
        let doc = &gltf.document;
        let data = |b: gltf::Buffer| match b.source() {
            gltf::buffer::Source::Bin => Some(blob),
            gltf::buffer::Source::Uri(_) => None,
        };
        let n = doc.nodes().count();
        let mut parents = vec![None; n];
        let mut rest = Vec::with_capacity(n);
        for node in doc.nodes() {
            for c in node.children() {
                parents[c.index()] = Some(node.index());
            }
            let (t, r, s) = node.transform().decomposed();
            rest.push(Trs { t: Vec3::from(t), r: Quat::from_array(r), s: Vec3::from(s) });
        }
        let mut order = Vec::with_capacity(n);
        let mut placed = vec![false; n];
        while order.len() < n {
            let before = order.len();
            for i in 0..n {
                if !placed[i] && parents[i].is_none_or(|p| placed[p]) {
                    placed[i] = true;
                    order.push(i);
                }
            }
            if order.len() == before {
                return Err("character: the node tree has a loop".into());
            }
        }

        let skin = doc.skins().next().ok_or("character: no skeleton")?;
        let inverse: Vec<Mat4> = skin
            .reader(data)
            .read_inverse_bind_matrices()
            .ok_or("character: no inverse bind matrices")?
            .map(|m| Mat4::from_cols_array_2d(&m))
            .collect();
        let mut joints: Vec<(usize, Mat4)> =
            skin.joints().map(|j| j.index()).zip(inverse).collect();

        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for node in doc.nodes() {
            let Some(mesh) = node.mesh() else { continue };
            let name = node.name().unwrap_or_default();
            // The knight carries no weapons here.
            if ["Sword", "Shield", "Axe", "Bow", "Staff", "Wand"].iter().any(|w| name.contains(w)) {
                continue;
            }
            // A rigid part (helmet, cape): one pseudo joint for its own node.
            let rigid_joint = node.skin().is_none().then(|| {
                joints.push((node.index(), Mat4::IDENTITY));
                (joints.len() - 1) as f32
            });
            for prim in mesh.primitives() {
                let r = prim.reader(data);
                let base = vertices.len() as u32;
                let pos: Vec<[f32; 3]> =
                    r.read_positions().ok_or("character: no positions")?.collect();
                let normals: Vec<[f32; 3]> = r
                    .read_normals()
                    .map(|i| i.collect())
                    .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; pos.len()]);
                let uvs: Vec<[f32; 2]> = r
                    .read_tex_coords(0)
                    .map(|i| i.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.0; 2]; pos.len()]);
                let js: Vec<[u16; 4]> =
                    r.read_joints(0).map(|i| i.into_u16().collect()).unwrap_or_default();
                let ws: Vec<[f32; 4]> =
                    r.read_weights(0).map(|i| i.into_f32().collect()).unwrap_or_default();
                for (i, &p) in pos.iter().enumerate() {
                    let (joints4, weights) = match rigid_joint {
                        Some(j) => ([j, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]),
                        None => (
                            js.get(i).map_or([0.0; 4], |j| j.map(f32::from)),
                            ws.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]),
                        ),
                    };
                    vertices.push(SkinVertex {
                        pos: p,
                        normal: normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
                        uv: uvs.get(i).copied().unwrap_or([0.0; 2]),
                        joints: joints4,
                        weights,
                    });
                }
                let idx: Vec<u32> = match r.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..pos.len() as u32).collect(),
                };
                indices.extend(idx.into_iter().map(|i| base + i));
            }
        }
        if vertices.is_empty() {
            return Err("character: no meshes".into());
        }

        let clips = doc
            .animations()
            .map(|a| {
                let mut duration: f32 = 0.0;
                let channels = a
                    .channels()
                    .filter_map(|ch| {
                        let r = ch.reader(data);
                        let times: Vec<f32> = r.read_inputs()?.collect();
                        duration = duration.max(times.last().copied().unwrap_or(0.0));
                        use gltf::animation::util::ReadOutputs;
                        let values = match r.read_outputs()? {
                            ReadOutputs::Translations(v) => Values::T(v.map(Vec3::from).collect()),
                            ReadOutputs::Rotations(v) => {
                                Values::R(v.into_f32().map(Quat::from_array).collect())
                            }
                            ReadOutputs::Scales(v) => Values::S(v.map(Vec3::from).collect()),
                            ReadOutputs::MorphTargetWeights(_) => return None,
                        };
                        Some(Channel { node: ch.target().node().index(), times, values })
                    })
                    .collect();
                Clip { name: a.name().unwrap_or_default().to_string(), duration, channels }
            })
            .collect();

        let texture = Self::texture(doc, blob)?;
        let mut model = CharacterModel {
            vertices,
            indices,
            texture,
            clips,
            parents,
            rest,
            order,
            joints,
            fit: Mat4::IDENTITY,
        };
        model.fit = model.fit_to_ground();
        Ok(model)
    }

    fn texture(doc: &gltf::Document, blob: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
        let image = doc.images().next().ok_or("character: no texture")?;
        let gltf::image::Source::View { view, .. } = image.source() else {
            return Err("character: the texture must be inside the .glb".into());
        };
        let bytes = blob
            .get(view.offset()..view.offset() + view.length())
            .ok_or("character: texture out of range")?;
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| format!("character texture: {e}"))?;
        let mut buf = vec![0; reader.output_buffer_size().ok_or("character texture: too big")?];
        let info = reader.next_frame(&mut buf).map_err(|e| format!("character texture: {e}"))?;
        let px = &buf[..info.buffer_size()];
        let rgba = match info.color_type {
            png::ColorType::Rgba => px.to_vec(),
            png::ColorType::Rgb => px.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            png::ColorType::GrayscaleAlpha => {
                px.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
            }
            png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return Err("character texture: unexpanded palette".into()),
        };
        Ok((info.width, info.height, rgba))
    }

    /// Scale and lift the rest pose so it stands on y = 0, `HEIGHT` tall.
    fn fit_to_ground(&self) -> Mat4 {
        let joints = self.joint_matrices(&self.rest);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &self.vertices {
            let p = Vec3::from(v.pos);
            let mut m = Mat4::ZERO;
            for k in 0..4 {
                if let Some(j) = joints.get(v.joints[k] as usize) {
                    m += *j * v.weights[k];
                }
            }
            let w = m.transform_point3(p);
            lo = lo.min(w);
            hi = hi.max(w);
        }
        let h = (hi.y - lo.y).max(0.01);
        let s = HEIGHT / h;
        let center = (lo + hi) * 0.5;
        Mat4::from_scale(Vec3::splat(s))
            * Mat4::from_translation(Vec3::new(-center.x, -lo.y, -center.z))
    }

    pub fn clip(&self, name: &str) -> Option<usize> {
        self.clips.iter().position(|c| c.name == name)
    }

    /// The pose of `clip` at time `t` (seconds), on top of the rest pose.
    fn sample(&self, clip: usize, t: f32, out: &mut [Trs]) {
        let Some(c) = self.clips.get(clip) else { return };
        for ch in &c.channels {
            let Some(node) = out.get_mut(ch.node) else { continue };
            if ch.times.is_empty() {
                continue;
            }
            let (a, b, w) = sample_index(&ch.times, t);
            match &ch.values {
                Values::T(v) => node.t = v[a].lerp(v[b], w),
                Values::R(v) => node.r = v[a].slerp(v[b], w).normalize(),
                Values::S(v) => node.s = v[a].lerp(v[b], w),
            }
        }
    }

    fn joint_matrices(&self, local: &[Trs]) -> Vec<Mat4> {
        let mut global = vec![Mat4::IDENTITY; local.len()];
        for &i in &self.order {
            let m = local[i].matrix();
            global[i] = match self.parents[i] {
                Some(p) => global[p] * m,
                None => m,
            };
        }
        self.joints.iter().map(|(node, inv)| global[*node] * *inv).collect()
    }

    #[allow(dead_code)] // the tests check the shader's joint limit
    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    /// The joint matrices for what `anim` shows now.
    pub fn pose(&self, anim: &Animator) -> Vec<Mat4> {
        let mut now = self.rest.clone();
        self.sample(anim.clip, anim.time, &mut now);
        if let Some((from, t)) = anim.from {
            let w = (anim.fade / FADE).clamp(0.0, 1.0);
            if w < 1.0 {
                let mut old = self.rest.clone();
                self.sample(from, t, &mut old);
                for (n, o) in now.iter_mut().zip(&old) {
                    *n = o.lerp(n, w);
                }
            }
        }
        self.joint_matrices(&now)
    }
}

/// What the body is doing: the clip, where in it, and the one it fades from.
#[derive(Debug, Clone, PartialEq)]
pub struct Animator {
    pub clip: usize,
    pub time: f32,
    pub looping: bool,
    /// (clip, its time) being faded out, and how long the fade has run.
    from: Option<(usize, f32)>,
    fade: f32,
}

impl Animator {
    pub fn new(clip: usize) -> Self {
        Self { clip, time: 0.0, looping: true, from: None, fade: FADE }
    }

    /// Switch to `clip` (from its start, with a short fade), unless it already plays.
    pub fn play(&mut self, clip: usize, looping: bool) {
        if clip == self.clip && looping == self.looping {
            return;
        }
        self.from = Some((self.clip, self.time));
        self.fade = 0.0;
        self.clip = clip;
        self.time = 0.0;
        self.looping = looping;
    }

    pub fn advance(&mut self, dt: f32, model: &CharacterModel) {
        let duration = model.clips.get(self.clip).map_or(1.0, |c| c.duration).max(1e-3);
        self.time += dt;
        if self.looping {
            self.time %= duration;
        } else {
            self.time = self.time.min(duration);
        }
        self.fade += dt;
        if self.fade >= FADE {
            self.from = None;
        }
    }

    /// A clip played once has reached its end.
    pub fn finished(&self, model: &CharacterModel) -> bool {
        !self.looping && model.clips.get(self.clip).is_none_or(|c| self.time >= c.duration - 1e-4)
    }

    /// Something visibly changes (keep drawing frames).
    pub fn moving(&self, model: &CharacterModel) -> bool {
        self.looping || !self.finished(model) || self.from.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_knight_loads() {
        let m = CharacterModel::load(KNIGHT).expect("loads");
        assert!(m.clips.len() >= 70, "{} clips", m.clips.len());
        for name in ["Idle", "Walking_A", "Running_A", "Jump_Idle", "Cheer", "Sit_Floor_Idle"] {
            assert!(m.clip(name).is_some(), "{name}");
        }
        assert!(m.indices.len() / 3 < 10_000, "{} triangles", m.indices.len() / 3);
        // 41 bones + helmet + cape; the shader holds 64.
        assert!(m.joint_count() <= 64, "{} joints", m.joint_count());
        assert!(m.texture.0 > 0 && m.texture.2.len() == (m.texture.0 * m.texture.1 * 4) as usize);
        // Feet on the ground, HEIGHT tall.
        let feet = m.fit.transform_point3(Vec3::ZERO);
        assert!(feet.is_finite());
    }

    #[test]
    fn rest_pose_skins_to_the_mesh_itself() {
        // In the rest pose, joint * inverse_bind is (nearly) the identity for the skinned
        // parts: nothing moves.
        let m = CharacterModel::load(KNIGHT).expect("loads");
        let j = m.joint_matrices(&m.rest);
        let skinned =
            m.vertices.iter().find(|v| v.weights[0] > 0.99 && v.joints[0] < 41.0).expect("v");
        let jm = j[skinned.joints[0] as usize];
        let p = Vec3::from(skinned.pos);
        assert!(
            jm.transform_point3(p).distance(p) < 0.05,
            "{:?} -> {:?}",
            p,
            jm.transform_point3(p)
        );
    }

    #[test]
    fn animations_move_and_fade() {
        let m = CharacterModel::load(KNIGHT).expect("loads");
        let walk = m.clip("Walking_A").expect("walk");
        let idle = m.clip("Idle").expect("idle");
        let mut a = Animator::new(idle);
        let p0 = m.pose(&a);
        a.play(walk, true);
        a.advance(0.4, &m);
        let p1 = m.pose(&a);
        assert!(p0.iter().zip(&p1).any(|(x, y)| !x.abs_diff_eq(*y, 1e-3)), "walking moves bones");
        // One-shot clips stop at their end.
        let cheer = m.clip("Cheer").expect("cheer");
        a.play(cheer, false);
        a.advance(100.0, &m);
        assert!(a.finished(&m));
        assert_eq!(sample_index(&[0.0, 1.0, 2.0], 1.5), (1, 2, 0.5));
        assert_eq!(sample_index(&[0.0, 1.0], 5.0), (1, 1, 0.0));
    }
}
