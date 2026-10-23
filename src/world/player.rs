//! The player: a box that walks, jumps, falls and collides with blocks.
//!
//! Physics runs at a fixed 120 steps per second, whatever the frame rate: the frame time
//! is added to an accumulator and whole steps are taken from it. With variable steps,
//! jump height and collision would change with FPS.
//!
//! Collision is axis-separated: move along Y, resolve overlaps; then X; then Z. Resolving
//! one axis at a time means you slide along walls instead of sticking to them, and you
//! can never end up inside a block diagonally.

use std::time::{Duration, Instant};

use glam::{IVec3, Vec3};

/// Player box: 0.6 × 1.8 × 0.6 m, eyes at 1.62 m.
pub const HALF_WIDTH: f32 = 0.3;
pub const HEIGHT: f32 = 1.8;
pub const EYE_HEIGHT: f32 = 1.62;
const GRAVITY: f32 = 25.0;
const MAX_FALL: f32 = 50.0;
pub const WALK_SPEED: f32 = 4.3;
pub const SPRINT_SPEED: f32 = 5.6;
const JUMP_SPEED: f32 = 8.0;
const FLY_SPEED: f32 = 10.0;
const STEP: f32 = 1.0 / 120.0;
/// Two Space presses this close together toggle flying.
const DOUBLE_TAP: Duration = Duration::from_millis(300);
/// A Space press is remembered this long: a tap shorter than one frame (or pressed a
/// moment before landing) still jumps ("jump buffering").
const JUMP_BUFFER: Duration = Duration::from_millis(200);
/// Tiny gap kept between the player and a block face, so we don't count as touching it.
const SKIN: f32 = 1e-3;

/// What the held keys ask for this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MoveIntent {
    /// −1..1 along the view direction (on the ground plane).
    pub forward: f32,
    /// −1..1 to the right.
    pub right: f32,
    /// Space held (jump / fly up / swim up).
    pub up: bool,
    /// Shift held (fly down).
    pub down: bool,
    pub sprint: bool,
}

#[derive(Debug, Clone)]
pub struct Player {
    /// Center of the feet.
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    pub flying: bool,
    /// m/s (from the config file in Stage 7).
    pub walk_speed: f32,
    pub sprint_speed: f32,
    accumulator: f32,
    last_space: Option<Instant>,
    jump_requested: Option<Instant>,
}

impl Player {
    pub fn new(pos: Vec3) -> Self {
        Self {
            pos,
            vel: Vec3::ZERO,
            on_ground: false,
            flying: false,
            walk_speed: WALK_SPEED,
            sprint_speed: SPRINT_SPEED,
            accumulator: 0.0,
            last_space: None,
            jump_requested: None,
        }
    }

    pub fn eye(&self) -> Vec3 {
        self.pos + Vec3::new(0.0, EYE_HEIGHT, 0.0)
    }

    pub fn aabb(&self) -> (Vec3, Vec3) {
        (
            self.pos - Vec3::new(HALF_WIDTH, 0.0, HALF_WIDTH),
            self.pos + Vec3::new(HALF_WIDTH, HEIGHT, HALF_WIDTH),
        )
    }

    /// Does the player's box overlap the block cell at `cell`?
    pub fn overlaps_block(&self, cell: IVec3) -> bool {
        let (min, max) = self.aabb();
        let (bmin, bmax) = (cell.as_vec3(), cell.as_vec3() + Vec3::ONE);
        min.x < bmax.x
            && max.x > bmin.x
            && min.y < bmax.y
            && max.y > bmin.y
            && min.z < bmax.z
            && max.z > bmin.z
    }

    /// Space was pressed: double tap toggles flying.
    pub fn space_pressed(&mut self, now: Instant) {
        self.jump_requested = Some(now);
        if self.last_space.is_some_and(|t| now.duration_since(t) <= DOUBLE_TAP) {
            self.flying = !self.flying;
            self.vel.y = 0.0;
            self.last_space = None;
            tracing::debug!(flying = self.flying, "fly toggled");
        } else {
            self.last_space = Some(now);
        }
    }

    /// Advance by `dt` seconds of real time. `forward_flat` is the horizontal view
    /// direction. Returns true if the player moved.
    pub fn update(
        &mut self,
        dt: f32,
        intent: MoveIntent,
        forward_flat: Vec3,
        solid: &impl Fn(IVec3) -> bool,
        in_water: &impl Fn(IVec3) -> bool,
    ) -> bool {
        let start = self.pos;
        // Never simulate more than a quarter second at once (after a stall).
        self.accumulator = (self.accumulator + dt).min(0.25);
        while self.accumulator >= STEP {
            self.accumulator -= STEP;
            self.step(intent, forward_flat, solid, in_water);
        }
        (self.pos - start).length_squared() > 1e-10
    }

    fn step(
        &mut self,
        intent: MoveIntent,
        forward_flat: Vec3,
        solid: &impl Fn(IVec3) -> bool,
        in_water: &impl Fn(IVec3) -> bool,
    ) {
        let right = forward_flat.cross(Vec3::Y);
        let mut wish = forward_flat * intent.forward + right * intent.right;
        if wish.length_squared() > 1.0 {
            wish = wish.normalize();
        }
        let swimming =
            !self.flying && in_water((self.pos + Vec3::new(0.0, 0.5, 0.0)).floor().as_ivec3());

        if self.flying {
            self.vel = wish * FLY_SPEED;
            self.vel.y = match (intent.up, intent.down) {
                (true, false) => FLY_SPEED,
                (false, true) => -FLY_SPEED,
                _ => 0.0,
            };
        } else {
            let speed = if intent.sprint { self.sprint_speed } else { self.walk_speed };
            self.vel.x = wish.x * speed;
            self.vel.z = wish.z * speed;
            if swimming {
                // Water: slow sinking, Space swims up.
                self.vel.y =
                    if intent.up { 3.0 } else { (self.vel.y - GRAVITY * 0.2 * STEP).max(-2.0) };
            } else {
                let buffered = self.jump_requested.is_some_and(|t| t.elapsed() <= JUMP_BUFFER);
                if (intent.up || buffered) && self.on_ground {
                    self.vel.y = JUMP_SPEED;
                    self.jump_requested = None;
                }
                self.vel.y = (self.vel.y - GRAVITY * STEP).max(-MAX_FALL);
            }
        }

        let delta = self.vel * STEP;
        self.on_ground = false;
        // Y first: landing must be resolved before sliding sideways.
        for axis in [1, 0, 2] {
            if delta[axis] == 0.0 {
                continue;
            }
            if self.move_axis(axis, delta[axis], solid) {
                if axis == 1 && delta[axis] < 0.0 {
                    self.on_ground = true;
                }
                self.vel[axis] = 0.0;
            }
        }
    }

    /// Move along one axis, then push out of any solid block. Returns true if blocked.
    fn move_axis(&mut self, axis: usize, delta: f32, solid: &impl Fn(IVec3) -> bool) -> bool {
        self.pos[axis] += delta;
        let (min, max) = self.aabb();
        let lo = (min - Vec3::splat(SKIN * 0.5)).floor().as_ivec3();
        let hi = (max - Vec3::splat(SKIN * 0.5)).floor().as_ivec3();
        let mut blocked = false;
        for x in lo.x..=hi.x {
            for y in lo.y..=hi.y {
                for z in lo.z..=hi.z {
                    let cell = IVec3::new(x, y, z);
                    if !solid(cell) || !self.overlaps_block(cell) {
                        continue;
                    }
                    blocked = true;
                    // Offset of the box's min corner from `pos` on this axis.
                    let (below, above) = match axis {
                        1 => (0.0, HEIGHT),
                        _ => (HALF_WIDTH, HALF_WIDTH),
                    };
                    if delta > 0.0 {
                        let limit = cell[axis] as f32 - above - SKIN;
                        self.pos[axis] = self.pos[axis].min(limit);
                    } else {
                        let limit = cell[axis] as f32 + 1.0 + below + SKIN;
                        self.pos[axis] = self.pos[axis].max(limit);
                    }
                }
            }
        }
        blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat floor: everything at y < 10 is solid.
    fn floor(c: IVec3) -> bool {
        c.y < 10
    }
    fn dry(_: IVec3) -> bool {
        false
    }

    fn run(p: &mut Player, seconds: f32, intent: MoveIntent, solid: &impl Fn(IVec3) -> bool) {
        let frames = (seconds * 60.0) as usize;
        for _ in 0..frames {
            p.update(1.0 / 60.0, intent, Vec3::NEG_Z, solid, &dry);
        }
    }

    #[test]
    fn falls_and_lands_on_the_ground() {
        let mut p = Player::new(Vec3::new(0.5, 20.0, 0.5));
        run(&mut p, 3.0, MoveIntent::default(), &floor);
        assert!(p.on_ground);
        assert!((p.pos.y - 10.0).abs() < 0.01, "feet on the floor top: {}", p.pos.y);
        assert_eq!(p.vel.y, 0.0);
    }

    #[test]
    fn jump_height_is_about_1_25_blocks() {
        let mut p = Player::new(Vec3::new(0.5, 10.0, 0.5));
        run(&mut p, 0.5, MoveIntent::default(), &floor);
        let mut peak: f32 = 0.0;
        let jump = MoveIntent { up: true, ..Default::default() };
        for i in 0..120 {
            let intent = if i == 0 { jump } else { MoveIntent::default() };
            p.update(1.0 / 60.0, intent, Vec3::NEG_Z, &floor, &dry);
            peak = peak.max(p.pos.y - 10.0);
        }
        // v²/2g = 64/50 = 1.28 m: one block up is reachable, two are not.
        assert!(peak > 1.1 && peak < 1.4, "peak {peak}");
        assert!(p.on_ground, "landed again");
    }

    #[test]
    fn walls_block_and_you_slide_along_them() {
        // Floor plus a wall at z = -3 (cells with z == -3 and y in 10..13).
        let solid = |c: IVec3| c.y < 10 || (c.z == -3 && (10..13).contains(&c.y));
        let mut p = Player::new(Vec3::new(0.5, 10.0, 0.5));
        run(&mut p, 2.0, MoveIntent { forward: 1.0, right: 0.5, ..Default::default() }, &solid);
        // Stopped in front of the wall (its face is at z = -2), but kept moving right.
        assert!((p.pos.z - (-2.0 + HALF_WIDTH)).abs() < 0.01, "z {}", p.pos.z);
        assert!(p.pos.x > 2.0, "slid along the wall: x {}", p.pos.x);
    }

    #[test]
    fn walking_speed_and_sprint() {
        let mut p = Player::new(Vec3::new(0.5, 10.0, 0.5));
        run(&mut p, 0.5, MoveIntent::default(), &floor);
        let start = p.pos;
        run(&mut p, 1.0, MoveIntent { forward: 1.0, ..Default::default() }, &floor);
        assert!(((start.z - p.pos.z) - WALK_SPEED).abs() < 0.1);
        let start = p.pos;
        run(&mut p, 1.0, MoveIntent { forward: 1.0, sprint: true, ..Default::default() }, &floor);
        assert!(((start.z - p.pos.z) - SPRINT_SPEED).abs() < 0.1);
    }

    #[test]
    fn double_tap_toggles_flying() {
        let mut p = Player::new(Vec3::new(0.5, 10.0, 0.5));
        let t0 = Instant::now();
        p.space_pressed(t0);
        assert!(!p.flying);
        p.space_pressed(t0 + Duration::from_millis(200));
        assert!(p.flying);
        // Flying: no gravity.
        let y = p.pos.y;
        run(&mut p, 1.0, MoveIntent::default(), &|_| false);
        assert!((p.pos.y - y).abs() < 1e-4);
        // Slow taps don't toggle.
        p.space_pressed(t0 + Duration::from_secs(5));
        p.space_pressed(t0 + Duration::from_secs(6));
        assert!(p.flying);
    }

    #[test]
    fn frame_rate_independent() {
        let mut a = Player::new(Vec3::new(0.5, 30.0, 0.5));
        let mut b = a.clone();
        let step = |p: &mut Player, fps: f32, seconds: f32| {
            for _ in 0..(fps * seconds).round() as usize {
                p.update(1.0 / fps, MoveIntent::default(), Vec3::NEG_Z, &floor, &dry);
            }
        };
        // Mid-fall at 30 vs 144 fps: the same fixed steps, give or take one step's worth
        // of motion from rounding in the accumulator (~12 m/s × 1/120 s).
        step(&mut a, 30.0, 0.5);
        step(&mut b, 144.0, 0.5);
        assert!((a.pos.y - b.pos.y).abs() < 0.25, "{} vs {}", a.pos.y, b.pos.y);
        // And they land on exactly the same spot.
        step(&mut a, 30.0, 2.0);
        step(&mut b, 144.0, 2.0);
        assert!((a.pos.y - b.pos.y).abs() < 1e-4 && a.on_ground && b.on_ground);
    }
}
