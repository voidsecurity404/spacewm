//! Terrain generation: a seeded, deterministic island in an ocean.
//!
//! Height comes from fractal value noise (several octaves of smooth random bumps added
//! together), multiplied by a falloff toward the island edge so hills give way to a sandy
//! beach and then the sea. The same seed always gives the same world.

use glam::{IVec3, Vec3};

use super::{block::Block, chunk::ChunkMap};

pub const DEFAULT_SEED: u64 = 1337;
/// The generated area is 256×256 blocks (−128..128): far enough that its edge lies
/// beyond the fog.
pub const WORLD_HALF: i32 = 128;
/// The island is about 96×96 blocks.
pub const ISLAND_RADIUS: f32 = 48.0;
pub const SEA_LEVEL: i32 = 6;
const BASE_HEIGHT: f32 = 8.0;
/// The flat stone "desk" at spawn where the first windows appear.
pub const PLATFORM_HALF: i32 = 4;
const TREE_SPACING: i32 = 7;

/// splitmix64: a fast, well-mixed integer hash. Turns (seed, x, z) into random bits.
fn hash(seed: u64, x: i32, z: i32, salt: u64) -> u64 {
    let mut v = seed
        ^ (x as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ salt.wrapping_mul(0x1656_67B1_9E37_79F9);
    v = (v ^ (v >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    v ^ (v >> 31)
}

/// Random value in 0..1 for an integer lattice point.
fn lattice(seed: u64, x: i32, z: i32, salt: u64) -> f32 {
    (hash(seed, x, z, salt) >> 40) as f32 / (1u64 << 24) as f32
}

/// Smooth noise: random values on integer points, blended with a smoothstep curve.
fn value_noise(seed: u64, x: f32, z: f32, salt: u64) -> f32 {
    let (x0, z0) = (x.floor() as i32, z.floor() as i32);
    let (fx, fz) = (x - x0 as f32, z - z0 as f32);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sz) = (s(fx), s(fz));
    let a = lattice(seed, x0, z0, salt);
    let b = lattice(seed, x0 + 1, z0, salt);
    let c = lattice(seed, x0, z0 + 1, salt);
    let d = lattice(seed, x0 + 1, z0 + 1, salt);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sz
}

/// Fractal Brownian motion: 4 octaves, each twice the frequency and half the amplitude.
/// Result in 0..1.
fn fbm(seed: u64, x: f32, z: f32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0 / 24.0, 0.0);
    for octave in 0..4 {
        sum += value_noise(seed, x * freq, z * freq, octave) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

/// Terrain surface height at a column.
pub fn height(seed: u64, x: i32, z: i32) -> i32 {
    let d = x.abs().max(z.abs());
    if d <= PLATFORM_HALF + 1 {
        return platform_height(seed);
    }
    let raw = raw_height(seed, x, z);
    if d >= CLEARING {
        return raw;
    }
    // Around the platform the ground eases from the platform height into the hills, so
    // the desk sits in a clearing instead of at the bottom of a cliff.
    let t = (d - PLATFORM_HALF - 1) as f32 / (CLEARING - PLATFORM_HALF - 1) as f32;
    let s = t * t * (3.0 - 2.0 * t);
    let p = platform_height(seed) as f32;
    (p + (raw as f32 - p) * s).round() as i32
}

/// Radius (in blocks, square) over which the terrain blends into the platform.
const CLEARING: i32 = 14;

/// Height before the platform is flattened in.
fn raw_height(seed: u64, x: i32, z: i32) -> i32 {
    let (fx, fz) = (x as f32, z as f32);
    // A wobbly coastline: the island radius itself varies with low-frequency noise, so
    // the island isn't a perfect circle (and the hills aren't concentric rings).
    let wobble = 0.75 + 0.5 * value_noise(seed, fx / 37.0, fz / 37.0, 11);
    let r = (fx * fx + fz * fz).sqrt() / (ISLAND_RADIUS * wobble);
    // 1 in the middle, smoothly 0 at the coast and beyond.
    let falloff = (1.0 - r * r).clamp(0.0, 1.0);
    // Hills: fractal noise, sharpened (squared) so there are distinct peaks and valleys
    // rather than one big dome.
    let n = fbm(seed, fx, fz);
    let hills = (n * n * 2.0) * 13.0 * falloff.sqrt() + 3.0 * falloff;
    // Outside the island the seabed slopes down to a few blocks below sea level.
    let sea_floor = -4.0 * (1.0 - falloff);
    (BASE_HEIGHT - 3.0 + hills + sea_floor).round() as i32
}

/// The desk platform sits at the average terrain height around it (never on the
/// beach), so it is a level patch of the hill instead of a tower or a pit.
fn platform_height(seed: u64) -> i32 {
    let r = PLATFORM_HALF + 2;
    let ring: Vec<i32> = (-r..=r)
        .flat_map(|x| (-r..=r).map(move |z| (x, z)))
        .filter(|&(x, z)| x.abs() == r || z.abs() == r)
        .map(|(x, z)| raw_height(seed, x, z))
        .collect();
    let mean = ring.iter().sum::<i32>() as f32 / ring.len() as f32;
    (mean.round() as i32).max(SEA_LEVEL + 2)
}

pub struct Terrain {
    pub map: ChunkMap,
    /// Where the player appears: on top of the platform, at its center.
    pub spawn: Vec3,
    /// Which way the player looks at first (camera yaw): toward the most open view.
    pub spawn_yaw: f32,
}

pub fn generate(seed: u64) -> Terrain {
    let mut map = ChunkMap::default();
    for x in -WORLD_HALF..WORLD_HALF {
        for z in -WORLD_HALF..WORLD_HALF {
            let h = height(seed, x, z).max(1);
            let on_platform = x.abs() <= PLATFORM_HALF && z.abs() <= PLATFORM_HALF;
            let beach = h <= SEA_LEVEL + 1;
            for y in 0..=h {
                let block = if on_platform && y == h {
                    Block::Stone
                } else if y == h {
                    if beach {
                        Block::Sand
                    } else {
                        Block::Grass
                    }
                } else if y >= h - 3 {
                    if beach {
                        Block::Sand
                    } else {
                        Block::Dirt
                    }
                } else {
                    Block::Stone
                };
                map.set_block_raw(IVec3::new(x, y, z), block);
            }
            for y in (h + 1)..=SEA_LEVEL {
                map.set_block_raw(IVec3::new(x, y, z), Block::Water);
            }
        }
    }
    plant_trees(seed, &mut map);
    let top = map.top_solid(0, 0, 0, 64).unwrap_or(SEA_LEVEL);
    let spawn_yaw = most_open_direction(&map, top);
    Terrain { map, spawn: Vec3::new(0.5, top as f32 + 1.0, 0.5), spawn_yaw }
}

/// Of 16 directions, the one where the ground (and trees) in front of the platform rise
/// the least above it — so the first thing you see is the island, not a wall of dirt.
fn most_open_direction(map: &ChunkMap, platform: i32) -> f32 {
    let mut best = (f32::MAX, 0.0);
    for i in 0..16 {
        let yaw = i as f32 * std::f32::consts::TAU / 16.0;
        // Camera yaw 0 looks toward -Z, positive turns toward +X.
        let (dx, dz) = (yaw.sin(), -yaw.cos());
        let blocked: f32 = (6..24)
            .map(|d| {
                let (x, z) = ((dx * d as f32).round() as i32, (dz * d as f32).round() as i32);
                let h = map.top_solid(x, z, 0, 64).unwrap_or(0);
                // Nearby obstacles block more of the view than distant ones.
                (h - platform - 1).max(0) as f32 / d as f32
            })
            .sum();
        if blocked < best.0 {
            best = (blocked, yaw);
        }
    }
    best.1
}

fn plant_trees(seed: u64, map: &mut ChunkMap) {
    let mut planted: Vec<(i32, i32)> = Vec::new();
    let limit = ISLAND_RADIUS as i32;
    for x in -limit..limit {
        for z in -limit..limit {
            // Keep the area around the desk clear.
            if x.abs() <= PLATFORM_HALF + 3 && z.abs() <= PLATFORM_HALF + 3 {
                continue;
            }
            if lattice(seed, x, z, 101) > 0.025 {
                continue;
            }
            let h = height(seed, x, z);
            if map.block_at(IVec3::new(x, h, z)) != Block::Grass {
                continue;
            }
            let crowded = planted
                .iter()
                .any(|&(px, pz)| (px - x).abs() < TREE_SPACING && (pz - z).abs() < TREE_SPACING);
            if crowded {
                continue;
            }
            planted.push((x, z));
            let trunk = 4 + (hash(seed, x, z, 202) % 3) as i32; // 4..=6
            for y in 1..=trunk {
                map.set_block_raw(IVec3::new(x, h + y, z), Block::Wood);
            }
            // A rounded blob of leaves around the top of the trunk.
            let top = h + trunk;
            for dy in -2..=1 {
                let radius: i32 = if dy == 1 { 1 } else { 2 };
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        let corner = dx.abs() == radius && dz.abs() == radius;
                        if corner
                            && (dy == 1 || lattice(seed, x + dx, z + dz, (303 + dy) as u64) < 0.5)
                        {
                            continue;
                        }
                        let p = IVec3::new(x + dx, top + dy, z + dz);
                        if map.block_at(p) == Block::Air {
                            map.set_block_raw(p, Block::Leaves);
                        }
                    }
                }
            }
            map.set_block_raw(IVec3::new(x, top + 2, z), Block::Leaves);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        for (x, z) in [(0, 0), (10, -20), (-45, 30)] {
            assert_eq!(height(42, x, z), height(42, x, z));
        }
        // Different seeds give different islands somewhere.
        assert!((-40..40).any(|x| height(1, x, 7) != height(2, x, 7)));
    }

    #[test]
    fn island_then_sea() {
        // The middle is above sea level, far away is under water.
        assert!(height(DEFAULT_SEED, 0, 0) > SEA_LEVEL);
        assert!(height(DEFAULT_SEED, 120, 120) < SEA_LEVEL);
    }

    #[test]
    fn noise_range() {
        for i in 0..1000 {
            let v = fbm(7, i as f32 * 0.37, i as f32 * -0.91);
            assert!((0.0..=1.0).contains(&v), "{v}");
        }
    }
}
