//! Saving where panels are and which blocks you changed, across restarts.
//!
//! The file (`$XDG_STATE_HOME/spacewm/layout.json`) holds one placement per panel (keyed
//! by app_id) and the block edits as a diff against the generated terrain — the terrain
//! itself is regenerated from the seed, so the file stays small.
//!
//! Writes are atomic: write a temp file, flush it to disk, then rename it over the old
//! one. A crash mid-write leaves either the old file or the new one, never half of each.

use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use glam::{IVec3, Quat, Vec3};
use serde::{Deserialize, Serialize};

use super::block::Block;

const VERSION: u32 = 1;
/// Placements of closed windows kept around for when they come back.
const MAX_REMEMBERED: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPanel {
    pub app_id: String,
    /// The title when it was saved: breaks ties between several windows of one app.
    #[serde(default)]
    pub title_hint: Option<String>,
    pub position: [f32; 3],
    /// Quaternion x, y, z, w.
    pub rotation: [f32; 4],
    pub pixels_per_unit: f32,
    #[serde(default)]
    pub hidden: bool,
}

impl SavedPanel {
    pub fn position(&self) -> Vec3 {
        Vec3::from(self.position)
    }
    pub fn rotation(&self) -> Quat {
        let q = Quat::from_array(self.rotation);
        if q.is_finite() && q.length() > 0.5 {
            q.normalize()
        } else {
            Quat::IDENTITY
        }
    }
    fn is_valid(&self) -> bool {
        self.position.iter().all(|v| v.is_finite() && v.abs() < 1e5)
            && self.pixels_per_unit.is_finite()
            && (50.0..=10_000.0).contains(&self.pixels_per_unit)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedBlock {
    pub pos: [i32; 3],
    pub block: Block,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub panels: Vec<SavedPanel>,
    #[serde(default)]
    pub blocks: Vec<SavedBlock>,
}

/// In-memory layout state.
#[derive(Default)]
pub struct Layout {
    pub path: Option<PathBuf>,
    /// Placements waiting for their window to (re)appear: loaded from the file, or left
    /// behind by windows that closed. Newest first.
    pub pool: Vec<SavedPanel>,
    /// Block edits relative to the generated terrain.
    pub edits: HashMap<IVec3, Block>,
    /// Something changed since the last save.
    pub dirty: bool,
}

impl Layout {
    /// Load from `path`. A missing file is an empty layout; a corrupt one is moved aside
    /// to `layout.json.bad` (so it can be inspected) and we start fresh.
    pub fn load(path: PathBuf) -> Self {
        let file = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<LayoutFile>(&text) {
                Ok(file) => file,
                Err(err) => {
                    let bad = path.with_extension("json.bad");
                    tracing::warn!(path = %path.display(), "corrupt layout ({err}); moved to {}", bad.display());
                    let _ = fs::rename(&path, &bad);
                    LayoutFile::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => LayoutFile::default(),
            Err(err) => {
                tracing::warn!(path = %path.display(), "cannot read layout: {err}");
                LayoutFile::default()
            }
        };
        let pool: Vec<SavedPanel> = file.panels.into_iter().filter(SavedPanel::is_valid).collect();
        let edits = file.blocks.into_iter().map(|b| (IVec3::from(b.pos), b.block)).collect();
        tracing::info!(path = %path.display(), panels = pool.len(), "layout loaded");
        Self { path: Some(path), pool, edits, dirty: false }
    }

    /// The saved placement for a new window of `app_id`, if there is an unused one. With
    /// several candidates, one whose title matches wins.
    pub fn take_placement(&mut self, app_id: &str, title: Option<&str>) -> Option<SavedPanel> {
        let matches = |p: &SavedPanel| p.app_id == app_id;
        let index = title
            .and_then(|t| {
                self.pool.iter().position(|p| matches(p) && p.title_hint.as_deref() == Some(t))
            })
            .or_else(|| self.pool.iter().position(matches))?;
        self.dirty = true;
        Some(self.pool.remove(index))
    }

    /// A window closed: remember where it was for next time.
    pub fn remember(&mut self, placement: SavedPanel) {
        self.pool.insert(0, placement);
        self.pool.truncate(MAX_REMEMBERED);
        self.dirty = true;
    }

    pub fn record_block(&mut self, pos: IVec3, block: Block) {
        self.edits.insert(pos, block);
        self.dirty = true;
    }

    /// Serialize the open panels plus everything still waiting in the pool.
    pub fn to_file(&self, open: Vec<SavedPanel>) -> LayoutFile {
        let mut panels = open;
        panels.extend(self.pool.iter().cloned());
        let mut blocks: Vec<SavedBlock> =
            self.edits.iter().map(|(p, b)| SavedBlock { pos: p.to_array(), block: *b }).collect();
        // Stable order: diffs stay readable and the file doesn't churn.
        blocks.sort_by_key(|b| (b.pos[1], b.pos[2], b.pos[0]));
        LayoutFile { version: VERSION, panels, blocks }
    }

    pub fn save(&mut self, open: Vec<SavedPanel>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        match write_atomic(&path, &self.to_file(open)) {
            Ok(()) => {
                self.dirty = false;
                tracing::debug!(path = %path.display(), "layout saved");
            }
            Err(err) => tracing::warn!(path = %path.display(), "saving layout failed: {err}"),
        }
    }
}

pub fn write_atomic(path: &Path, file: &LayoutFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        let text = serde_json::to_string_pretty(file).map_err(std::io::Error::other)?;
        f.write_all(text.as_bytes())?;
        // Make sure the data is on disk before the rename makes it "the" file.
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("spacewm-layout-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("tmp dir");
        dir
    }

    fn panel(app: &str, x: f32, title: Option<&str>) -> SavedPanel {
        SavedPanel {
            app_id: app.into(),
            title_hint: title.map(Into::into),
            position: [x, 10.0, -3.0],
            rotation: [0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2],
            pixels_per_unit: 800.0,
            hidden: false,
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tmp_dir("roundtrip");
        let path = dir.join("layout.json");
        let mut layout = Layout::load(path.clone());
        assert!(layout.pool.is_empty());
        layout.record_block(IVec3::new(-3, 7, 2), Block::Glass);
        layout.record_block(IVec3::new(1, 9, 1), Block::Air);
        layout.save(vec![panel("foot", 1.0, None)]);
        assert!(!dir.join("layout.json.tmp").exists(), "temp file renamed away");

        let mut again = Layout::load(path);
        assert_eq!(again.edits.get(&IVec3::new(-3, 7, 2)), Some(&Block::Glass));
        assert_eq!(again.edits.get(&IVec3::new(1, 9, 1)), Some(&Block::Air));
        let p = again.take_placement("foot", None).expect("restored");
        assert_eq!(p.position, [1.0, 10.0, -3.0]);
        assert!(again.take_placement("foot", None).is_none(), "each entry is used once");
    }

    #[test]
    fn windows_of_one_app_restore_in_order_and_by_title() {
        let mut layout = Layout {
            pool: vec![
                panel("foot", 1.0, Some("htop")),
                panel("foot", 2.0, Some("vim")),
                panel("firefox", 3.0, None),
            ],
            ..Layout::default()
        };
        assert_eq!(layout.take_placement("foot", Some("vim")).map(|p| p.position[0]), Some(2.0));
        assert_eq!(layout.take_placement("foot", None).map(|p| p.position[0]), Some(1.0));
        assert!(layout.take_placement("foot", None).is_none());
        assert!(layout.take_placement("firefox", None).is_some());
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tmp_dir("corrupt");
        let path = dir.join("layout.json");
        fs::write(&path, "{ this is not json").expect("write");
        let layout = Layout::load(path.clone());
        assert!(layout.pool.is_empty());
        assert!(!path.exists());
        assert!(dir.join("layout.json.bad").exists());
    }

    #[test]
    fn invalid_entries_are_dropped() {
        let dir = tmp_dir("invalid");
        let path = dir.join("layout.json");
        let mut bad = panel("foot", 1.0, None);
        bad.pixels_per_unit = 0.0;
        let mut file =
            LayoutFile { version: 1, panels: vec![bad, panel("foot", 2.0, None)], blocks: vec![] };
        file.panels[1].rotation = [0.0, 0.0, 0.0, 0.0];
        write_atomic(&path, &file).expect("write");
        let mut layout = Layout::load(path);
        assert_eq!(layout.pool.len(), 1);
        assert_eq!(layout.take_placement("foot", None).map(|p| p.rotation()), Some(Quat::IDENTITY));
    }
}
