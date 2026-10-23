//! Block types and their properties.

use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Block {
    #[default]
    Air = 0,
    Grass,
    Dirt,
    Stone,
    Wood,
    Leaves,
    Glass,
    Sand,
    Water,
}

/// Texture layers (see render/atlas.rs).
pub mod tile {
    pub const GRASS_TOP: u16 = 0;
    pub const GRASS_SIDE: u16 = 1;
    pub const DIRT: u16 = 2;
    pub const STONE: u16 = 3;
    pub const WOOD_SIDE: u16 = 4;
    pub const WOOD_TOP: u16 = 5;
    pub const LEAVES: u16 = 6;
    pub const GLASS: u16 = 7;
    pub const SAND: u16 = 8;
    pub const WATER: u16 = 9;
    pub const COUNT: u16 = 10;
}

/// How a block is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderKind {
    Invisible,
    /// Fully opaque: hides whatever is behind it.
    Opaque,
    /// Opaque pixels or holes, never half-transparent (leaves): drawn with the opaque
    /// blocks using an alpha test, which keeps the depth buffer correct.
    Cutout,
    /// Half-transparent (glass, water): a second, sorted, blended pass.
    Translucent,
}

pub struct BlockProps {
    /// Collides with the player.
    pub solid: bool,
    pub render: RenderKind,
    /// Texture layers: top, side, bottom.
    pub tiles: [u16; 3],
}

const fn props(solid: bool, render: RenderKind, tiles: [u16; 3]) -> BlockProps {
    BlockProps { solid, render, tiles }
}

use tile::*;
const TABLE: [BlockProps; 9] = [
    props(false, RenderKind::Invisible, [0, 0, 0]), // Air
    props(true, RenderKind::Opaque, [GRASS_TOP, GRASS_SIDE, DIRT]), // Grass
    props(true, RenderKind::Opaque, [DIRT, DIRT, DIRT]), // Dirt
    props(true, RenderKind::Opaque, [STONE, STONE, STONE]), // Stone
    props(true, RenderKind::Opaque, [WOOD_TOP, WOOD_SIDE, WOOD_TOP]), // Wood
    props(true, RenderKind::Cutout, [LEAVES, LEAVES, LEAVES]), // Leaves
    props(true, RenderKind::Translucent, [GLASS, GLASS, GLASS]), // Glass
    props(true, RenderKind::Opaque, [SAND, SAND, SAND]), // Sand
    props(false, RenderKind::Translucent, [WATER, WATER, WATER]), // Water
];

impl Block {
    /// The blocks the number keys select, in order (1 = Grass ... 8 = Water).
    pub const PLACEABLE: [Block; 8] = [
        Block::Grass,
        Block::Dirt,
        Block::Stone,
        Block::Wood,
        Block::Leaves,
        Block::Glass,
        Block::Sand,
        Block::Water,
    ];

    pub fn props(self) -> &'static BlockProps {
        &TABLE[self as usize]
    }

    pub fn is_solid(self) -> bool {
        self.props().solid
    }

    /// Light passes through: neighbors behind it must still show their faces.
    pub fn is_transparent(self) -> bool {
        self.props().render != RenderKind::Opaque
    }

    #[cfg(test)]
    fn from_u8(v: u8) -> Block {
        match v {
            1 => Block::Grass,
            2 => Block::Dirt,
            3 => Block::Stone,
            4 => Block::Wood,
            5 => Block::Leaves,
            6 => Block::Glass,
            7 => Block::Sand,
            8 => Block::Water,
            _ => Block::Air,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_enum() {
        for v in 0..9u8 {
            assert_eq!(Block::from_u8(v) as u8, v);
        }
        assert!(!Block::Air.is_solid() && Block::Air.is_transparent());
        assert!(Block::Stone.is_solid() && !Block::Stone.is_transparent());
        assert!(!Block::Water.is_solid() && Block::Water.is_transparent());
        assert!(Block::Glass.is_solid() && Block::Glass.is_transparent());
        assert_eq!(Block::Grass.props().tiles, [tile::GRASS_TOP, tile::GRASS_SIDE, tile::DIRT]);
    }
}
