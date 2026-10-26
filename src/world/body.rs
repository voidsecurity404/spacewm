//! Your body in the world: F5 cycles the view (your eyes, behind you, in front of you),
//! and in the third-person views you see the character (world/character.rs) walk, run,
//! jump and stand around. F6 picks an emote (render/emotes.rs) it then plays.
//!
//! The view moves, not you: aiming, clicking windows and carrying them still go from
//! your eyes, like in most games; only the picture is taken from behind (or in front)
//! of you. The camera never goes into a wall: a ray from your eyes back toward where it
//! wants to be stops it short of anything solid.
//!
//! Cost: in your own eyes nothing is drawn or animated. In third person, walking draws
//! every frame anyway; standing still the idle animation runs at 15 frames a second.

use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use glam::{Mat4, Quat, Vec3};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use super::character::{Animator, CharacterModel, KNIGHT};
use crate::{input::raycast::Ray, render::character::CharacterDraw, state::SpaceState};

/// How far behind / in front of you the third-person views are.
const BACK_DISTANCE: f32 = 3.2;
const FRONT_DISTANCE: f32 = 2.6;
/// The third-person camera is this much above your eyes, looking this much (radians)
/// further down.
const LIFT: f32 = 0.35;
const TILT_DOWN: f32 = 0.16;
/// The camera stays this far from a wall it would otherwise go into.
const WALL_MARGIN: f32 = 0.25;
/// Standing still in third person: the idle animation's frame rate.
const IDLE_FRAME: Duration = Duration::from_millis(66);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Your own eyes (no body drawn).
    #[default]
    Eyes,
    Behind,
    Front,
}

/// The emotes in the F6 menu: label, clip, then (sitting, lying) the clip that loops
/// after it; or `hold`: stay in its last pose until you move.
#[derive(Debug, Clone, Copy)]
pub struct Emote {
    pub label: &'static str,
    pub clip: &'static str,
    pub then: Option<&'static str>,
    pub hold: bool,
}

pub const EMOTES: [Emote; 9] = [
    Emote { label: "Cheer", clip: "Cheer", then: None, hold: false },
    Emote { label: "Sit", clip: "Sit_Floor_Down", then: Some("Sit_Floor_Idle"), hold: false },
    Emote { label: "Lie down", clip: "Lie_Down", then: Some("Lie_Idle"), hold: false },
    Emote { label: "Punch", clip: "Unarmed_Melee_Attack_Punch_A", then: None, hold: false },
    Emote { label: "Kick", clip: "Unarmed_Melee_Attack_Kick", then: None, hold: false },
    Emote { label: "Magic", clip: "Spellcast_Long", then: None, hold: false },
    Emote { label: "Roll", clip: "Dodge_Forward", then: None, hold: false },
    Emote { label: "Pick up", clip: "PickUp", then: None, hold: false },
    Emote { label: "Fall down", clip: "Death_A", then: None, hold: true },
];

#[derive(Default)]
pub struct Body {
    pub mode: ViewMode,
    model: Option<Rc<CharacterModel>>,
    anim: Option<Animator>,
    /// The emote playing (index into EMOTES) and whether it reached its loop.
    emote: Option<(usize, bool)>,
    /// What the renderer shows this frame (None: nothing, you see through your eyes).
    pub draw: Option<CharacterDraw>,
    /// The camera the picture is taken from: position, yaw, pitch.
    pub view: Option<(Vec3, f32, f32)>,
    last_pose: Option<Instant>,
    idle_timer: bool,
    /// The model failed to load once: don't try again every frame.
    failed: bool,
}

impl Body {
    pub fn model(&self) -> Option<&CharacterModel> {
        self.model.as_deref()
    }

    /// Walking, jumping or an emote that isn't standing still: draw every frame.
    pub fn busy(&self) -> bool {
        self.draw.is_some()
            && self.model.as_deref().zip(self.anim.as_ref()).is_some_and(|(m, a)| {
                !(a.looping
                    && self.emote.is_none()
                    && a.clip == m.clip("Idle").unwrap_or(usize::MAX))
                    && a.moving(m)
            })
    }
}

impl SpaceState {
    /// F5: eyes -> behind -> in front -> eyes.
    pub fn cycle_view(&mut self) {
        self.body.mode = match self.body.mode {
            ViewMode::Eyes => ViewMode::Behind,
            ViewMode::Behind => ViewMode::Front,
            ViewMode::Front => ViewMode::Eyes,
        };
        tracing::info!(mode = ?self.body.mode, "view");
        self.view.dirty = true;
        self.queue_redraw();
    }

    /// Play emote `i` (from the F6 menu). In your own eyes you couldn't see it: the view
    /// goes in front of you.
    pub fn play_emote(&mut self, i: usize) {
        let Some(e) = EMOTES.get(i) else { return };
        if !self.ensure_body_model() {
            return;
        }
        if self.body.mode == ViewMode::Eyes {
            self.body.mode = ViewMode::Front;
        }
        let model = self.body.model.clone();
        if let (Some(m), Some(a)) = (model.as_deref(), self.body.anim.as_mut()) {
            if let Some(clip) = m.clip(e.clip) {
                a.play(clip, false);
                self.body.emote = Some((i, false));
                tracing::info!(emote = e.label, "emote");
            }
        }
        self.view.dirty = true;
        self.queue_redraw();
    }

    fn ensure_body_model(&mut self) -> bool {
        if self.body.model.is_some() {
            return true;
        }
        if self.body.failed {
            return false;
        }
        match CharacterModel::load(KNIGHT) {
            Ok(m) => {
                let idle = m.clip("Idle").unwrap_or(0);
                self.body.anim = Some(Animator::new(idle));
                self.body.model = Some(Rc::new(m));
                true
            }
            Err(e) => {
                tracing::error!("character not loaded: {e}");
                self.body.failed = true;
                false
            }
        }
    }

    /// Every frame (after the player moved): pick the animation, pose the body and put
    /// the third-person camera.
    pub fn update_body(&mut self, dt: f32) {
        if self.body.mode == ViewMode::Eyes
            || self.focus_view.is_some()
            || self.fullscreen.is_some()
        {
            if self.body.draw.take().is_some() || self.body.view.take().is_some() {
                self.view.dirty = true;
            }
            return;
        }
        if !self.ensure_body_model() {
            return;
        }
        let Some(model) = self.body.model.clone() else { return };
        let Some(mut anim) = self.body.anim.take() else { return };
        self.animate_body(&model, &mut anim, dt);
        self.body.anim = Some(anim);
    }

    fn animate_body(&mut self, model: &CharacterModel, anim: &mut Animator, dt: f32) {
        let walking = self.player.vel.x.hypot(self.player.vel.z) > 0.3;
        let airborne = !self.player.on_ground && !self.player.flying;
        // Moving ends an emote.
        if walking || airborne {
            self.body.emote = None;
        }
        if let Some((i, false)) = self.body.emote {
            if anim.finished(model) {
                let e = EMOTES[i];
                match e.then.and_then(|c| model.clip(c)) {
                    Some(next) => {
                        anim.play(next, true);
                        self.body.emote = Some((i, true));
                    }
                    None if e.hold => self.body.emote = Some((i, true)),
                    None => self.body.emote = None,
                }
            }
        }
        if self.body.emote.is_none() {
            let f = self.view.camera.forward_flat();
            let back = Vec3::new(self.player.vel.x, 0.0, self.player.vel.z).dot(f) < -0.3;
            let sprint = self.player.vel.x.hypot(self.player.vel.z) > self.player.walk_speed + 0.3;
            let name = match (airborne, walking, back, sprint) {
                (true, ..) => "Jump_Idle",
                (_, true, true, _) => "Walking_Backwards",
                (_, true, _, true) => "Running_A",
                (_, true, ..) => "Walking_A",
                _ => "Idle",
            };
            if let Some(clip) = model.clip(name) {
                anim.play(clip, true);
            }
        }

        // Standing still: the idle animation at 15 fps (a timer wakes us), not 60.
        let idle = !walking && !airborne && self.body.emote.is_none();
        let due = self.body.last_pose.is_none_or(|t| t.elapsed() >= IDLE_FRAME);
        let camera = self.third_person_camera();
        if idle && !due && camera == self.body.view && self.body.draw.is_some() {
            self.arm_idle_timer();
            return;
        }
        let step = match self.body.last_pose {
            Some(t) => t.elapsed().as_secs_f32().min(0.1),
            None => dt,
        };
        anim.advance(step, model);
        let joints = model.pose(anim);
        self.body.last_pose = Some(Instant::now());
        // The body faces where you look: glTF models face +Z, our yaw 0 looks at -Z.
        let yaw = self.view.camera.yaw;
        let placement = Mat4::from_rotation_translation(
            Quat::from_rotation_y(std::f32::consts::PI - yaw),
            self.player.pos,
        );
        self.body.draw = Some(CharacterDraw { joints, model: placement * model.fit });
        self.body.view = camera;
        self.view.dirty = true;
        if idle {
            self.arm_idle_timer();
        }
    }

    fn arm_idle_timer(&mut self) {
        if self.body.idle_timer {
            return;
        }
        self.body.idle_timer = true;
        let _ = self.loop_handle.insert_source(Timer::from_duration(IDLE_FRAME), |_, _, state| {
            state.body.idle_timer = false;
            if state.body.draw.is_some() {
                state.queue_redraw();
            }
            TimeoutAction::Drop
        });
    }

    /// Where the picture is taken from in the third-person views (None: your eyes).
    fn third_person_camera(&self) -> Option<(Vec3, f32, f32)> {
        let cam = &self.view.camera;
        let eye = self.player.eye();
        let forward = cam.forward();
        let (dir, want, yaw, pitch) = match self.body.mode {
            ViewMode::Eyes => return None,
            ViewMode::Behind => (-forward, BACK_DISTANCE, cam.yaw, cam.pitch),
            // In front, looking back at you.
            ViewMode::Front => {
                (forward, FRONT_DISTANCE, cam.yaw + std::f32::consts::PI, -cam.pitch)
            }
        };
        let ray = Ray { origin: eye, dir };
        let distance = match self.world_ray(&ray, want + WALL_MARGIN) {
            Some(hit) => (hit.t - WALL_MARGIN).max(0.3),
            None => want,
        };
        // A little higher and looking a little down: the whole body is in the picture,
        // clear of the dock at the bottom.
        let pos = eye + dir * distance + Vec3::new(0.0, LIFT, 0.0);
        Some((pos, yaw, (pitch - TILT_DOWN).max(-1.5)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_emote_exists_in_the_model() {
        let m = CharacterModel::load(KNIGHT).expect("loads");
        for e in EMOTES {
            assert!(m.clip(e.clip).is_some(), "{}", e.clip);
            if let Some(t) = e.then {
                assert!(m.clip(t).is_some(), "{t}");
            }
        }
        for name in ["Idle", "Walking_A", "Walking_Backwards", "Running_A", "Jump_Idle"] {
            assert!(m.clip(name).is_some(), "{name}");
        }
    }
}
