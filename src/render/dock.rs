//! The dock: favorite apps and the apps that are open, along the bottom while walking.
//!
//! Walking, the mouse turns the view; holding the Mod key frees an ordinary cursor (like
//! "hold Alt for the cursor" in games) to click the dock:
//! - left click: an open app's window comes in front of you (again: the next one); an
//!   app that isn't open starts;
//! - right click: always start a new one.
//!
//! Open apps have a dot under them. Hovering shows the app's name.

use std::{collections::HashMap, time::Instant};

use glam::Vec3;
use smithay::utils::{Logical, Point};

use super::text::{self, Style};
use crate::{
    input::mode::{set_mode, InputMode},
    inventory::{draw::Canvas, Image},
    launcher::desktop::DesktopEntry,
    state::SpaceState,
    world::panel::PanelId,
};

/// Layout in logical px.
const ICON: i32 = 40;
const SLOT: i32 = 52;
const PAD: i32 = 6;
const SEP: i32 = 14;
const DOT: i32 = 6;
pub const MARGIN: i32 = 14;

#[derive(Debug, Clone, PartialEq)]
pub struct DockItem {
    pub name: String,
    pub entry: Option<DesktopEntry>,
    pub icon: Option<String>,
    /// Open windows of this app, oldest first.
    pub windows: Vec<PanelId>,
    pub favorite: bool,
}

/// What the image was drawn for: (app name, open) per slot, hover, scale.
type Drawn = (Vec<(String, bool)>, Option<usize>, i32);

#[derive(Default)]
pub struct Dock {
    pub entries: Option<Vec<DesktopEntry>>,
    entries_loaded: Option<Instant>,
    icons: HashMap<String, Option<Vec<u8>>>,
    pub items: Vec<DockItem>,
    pub hover: Option<usize>,
    /// Mod is held: the mouse moves a cursor instead of the view.
    pub cursor: bool,
    /// The cursor moved since Mod went down (only then is it drawn: tapping Mod for a
    /// key combo must not flash a cursor).
    pub cursor_moved: bool,
    /// Which window of an app the next click brings (cycles through them).
    cycle: HashMap<String, usize>,
    pub image: Option<Image>,
    pub tooltip: Option<Image>,
    /// Where they go (output-relative), worked out with the images.
    pub origin: (i32, i32),
    pub tooltip_at: (i32, i32),
    drawn: Option<Drawn>,
}

/// "firefox" for "firefox.desktop".
fn stem(id: &str) -> &str {
    id.strip_suffix(".desktop").unwrap_or(id)
}

fn program(entry: &DesktopEntry) -> String {
    crate::desktop::launching::display_name(&entry.exec)
}

/// The installed app a dock setting names: its file name, program or display name.
pub fn entry_for_spec<'a>(entries: &'a [DesktopEntry], spec: &str) -> Option<&'a DesktopEntry> {
    let eq = |a: &str| a.eq_ignore_ascii_case(spec);
    entries
        .iter()
        .find(|e| eq(stem(&e.id)))
        .or_else(|| entries.iter().find(|e| eq(&program(e))))
        .or_else(|| entries.iter().find(|e| eq(&e.name)))
}

/// The installed app a window belongs to, from its app_id / X11 class.
pub fn entry_for_app<'a>(entries: &'a [DesktopEntry], app_id: &str) -> Option<&'a DesktopEntry> {
    let eq = |a: &str| a.eq_ignore_ascii_case(app_id);
    // "org.telegram.desktop" vs "telegram"; "com.microsoft.VSCode" vs "code".
    let last = app_id.rsplit('.').next().unwrap_or(app_id);
    entries
        .iter()
        .find(|e| eq(stem(&e.id)) || e.wm_class.as_deref().is_some_and(eq))
        .or_else(|| entries.iter().find(|e| eq(&program(e))))
        .or_else(|| {
            entries.iter().find(|e| {
                stem(&e.id).eq_ignore_ascii_case(last) || program(e).eq_ignore_ascii_case(last)
            })
        })
}

/// Favorites first, then other open apps; windows grouped by app.
pub fn build_items(
    entries: &[DesktopEntry],
    favorites: &[String],
    windows: &[(PanelId, String)],
) -> Vec<DockItem> {
    let mut items: Vec<DockItem> = Vec::new();
    for spec in favorites {
        if let Some(e) = entry_for_spec(entries, spec) {
            if !items.iter().any(|i| i.entry.as_ref() == Some(e)) {
                items.push(DockItem {
                    name: e.name.clone(),
                    entry: Some(e.clone()),
                    icon: e.icon.clone(),
                    windows: Vec::new(),
                    favorite: true,
                });
            }
        }
    }
    for (panel, app_id) in windows {
        let entry = entry_for_app(entries, app_id);
        let existing = items.iter_mut().find(|i| match (&i.entry, entry) {
            (Some(a), Some(b)) => a == b,
            (None, None) => i.name == *app_id,
            _ => false,
        });
        match existing {
            Some(item) => item.windows.push(*panel),
            None => items.push(DockItem {
                name: entry.map_or_else(|| app_id.clone(), |e| e.name.clone()),
                entry: entry.cloned(),
                icon: entry.and_then(|e| e.icon.clone()),
                windows: vec![*panel],
                favorite: false,
            }),
        }
    }
    items
}

/// Left edge of each slot, and the dock width.
fn slots(items: &[DockItem]) -> (Vec<i32>, i32) {
    let mut x = PAD;
    let mut xs = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        if i > 0 && items[i - 1].favorite && !item.favorite {
            x += SEP;
        }
        xs.push(x);
        x += SLOT;
    }
    (xs, x + PAD)
}

/// X of a slot's middle, relative to the dock.
pub fn slot_center(items: &[DockItem], i: usize) -> i32 {
    slots(items).0.get(i).map_or(0, |x| x + SLOT / 2)
}

pub fn dock_size(items: &[DockItem]) -> (i32, i32) {
    (slots(items).1, SLOT + 2 * PAD + DOT)
}

/// Where the dock goes on an output of `output` size.
pub fn dock_origin(items: &[DockItem], output: (i32, i32)) -> (i32, i32) {
    let (w, h) = dock_size(items);
    ((output.0 - w) / 2, output.1 - h - MARGIN)
}

/// The slot under a point relative to the dock's top-left corner.
pub fn item_at(items: &[DockItem], p: (f64, f64)) -> Option<usize> {
    let (xs, w) = slots(items);
    let h = dock_size(items).1;
    if p.1 < 0.0 || p.1 >= h as f64 || p.0 < 0.0 || p.0 >= w as f64 {
        return None;
    }
    xs.iter().position(|&x| p.0 >= x as f64 && p.0 < (x + SLOT) as f64)
}

/// A colored circle with the app's first letter, for apps without a PNG icon.
fn letter_icon(c: &mut Canvas, name: &str, x: i32, y: i32, size: i32, s: i32) {
    let hue = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let palette = [
        [0.85, 0.35, 0.30, 1.0],
        [0.30, 0.60, 0.90, 1.0],
        [0.35, 0.70, 0.40, 1.0],
        [0.80, 0.55, 0.20, 1.0],
        [0.60, 0.40, 0.80, 1.0],
        [0.25, 0.65, 0.65, 1.0],
    ];
    let color = palette[(hue as usize) % palette.len()];
    let r = size as f32 / 2.0;
    for yy in 0..size {
        for xx in 0..size {
            let (dx, dy) = (xx as f32 + 0.5 - r, yy as f32 + 0.5 - r);
            if dx * dx + dy * dy <= r * r {
                c.fill(x + xx, y + yy, 1, 1, color);
            }
        }
    }
    let letter: String =
        name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
    let w = Canvas::text_width(&letter, 22.0, s);
    c.text(&letter, 22.0, [1.0; 4], x + (size - w) / 2, y + size / 2 - 15 * s, s);
}

impl SpaceState {
    fn dock_enabled(&self) -> bool {
        self.config.dock.enabled
            && self.render_mode == crate::cli::RenderMode::Scene3D
            && self.mode == InputMode::Walk
            && self.fullscreen.is_none()
            && self.inventory.is_none()
            && !self.lock.is_locked()
            && !self.building_recently()
    }

    /// Before each frame: rebuild the items and the image when something changed.
    pub fn refresh_dock(&mut self) {
        if !self.dock_enabled() {
            if self.dock.image.is_some() {
                self.dock.image = None;
                self.dock.tooltip = None;
                self.dock.drawn = None;
            }
            return;
        }
        // The app list is re-read now and then (newly installed apps).
        self.ensure_entries();
        let windows: Vec<(PanelId, String)> = self
            .world
            .panels()
            .iter()
            .filter_map(|p| {
                let app = crate::state::window_names(&p.window).0.or_else(|| p.names.0.clone())?;
                Some((p.id, app))
            })
            .collect();
        let entries = self.dock.entries.as_deref().unwrap_or(&[]);
        let items = build_items(entries, &self.config.dock.apps, &windows);
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        let key: Vec<(String, bool)> =
            items.iter().map(|i| (i.name.clone(), !i.windows.is_empty())).collect();
        let hover = self.dock.hover.filter(|&h| h < items.len() && self.dock.cursor);
        self.dock.items = items;
        if let Some(geo) = self.primary_output_geometry() {
            self.dock.origin = dock_origin(&self.dock.items, (geo.size.w, geo.size.h));
        }
        let wanted = (key, hover, scale);
        if self.dock.drawn.as_ref() == Some(&wanted) {
            return;
        }
        tracing::debug!(items = ?wanted.0, "dock");
        self.dock.image = self.draw_dock(hover, scale);
        self.dock.tooltip = hover.and_then(|h| {
            let style = Style {
                size: 13.0,
                fg: [1.0; 4],
                bg: [0.05, 0.06, 0.08, 0.85],
                padding: 5,
                max_width: 500,
            };
            text::render(&[self.dock.items[h].name.clone()], style, scale)
                .map(|img| (img.buffer(), (img.width / img.scale, img.height / img.scale)))
        });
        if let (Some(h), Some((_, (tw, th)))) = (hover, &self.dock.tooltip) {
            let x = self.dock.origin.0 + slot_center(&self.dock.items, h) - tw / 2;
            self.dock.tooltip_at = (x.max(0), self.dock.origin.1 - th - 6);
        }
        self.dock.drawn = Some(wanted);
    }

    fn draw_dock(&mut self, hover: Option<usize>, scale: i32) -> Option<Image> {
        if self.dock.items.is_empty() {
            return None;
        }
        let s = scale;
        let (w, h) = dock_size(&self.dock.items);
        let (xs, _) = slots(&self.dock.items);
        let mut c = Canvas::new(w * s, h * s);
        let lifted = self.dock.cursor && self.dock.cursor_moved;
        let bg = if lifted { [0.10, 0.11, 0.14, 0.88] } else { [0.05, 0.06, 0.08, 0.55] };
        c.fill(0, 0, w * s, h * s, bg);
        c.frame(0, 0, w * s, h * s, s, [0.32, 0.36, 0.42, 0.9]);
        let icon_px = (ICON * s) as u32;
        for (i, item) in self.dock.items.clone().iter().enumerate() {
            let x = xs[i];
            if hover == Some(i) {
                c.fill(x * s, PAD * s, SLOT * s, SLOT * s, [0.25, 0.45, 0.75, 0.9]);
            }
            let ix = (x + (SLOT - ICON) / 2) * s;
            let iy = (PAD + (SLOT - ICON) / 2) * s;
            let pixels = item.icon.as_ref().and_then(|name| {
                self.dock
                    .icons
                    .entry(format!("{name}@{icon_px}"))
                    .or_insert_with(|| crate::desktop::icons::load(name, icon_px))
                    .clone()
            });
            match pixels {
                Some(px) => c.blit(&px, icon_px as i32, icon_px as i32, ix, iy),
                None => letter_icon(&mut c, &item.name, ix, iy, ICON * s, s),
            }
            if !item.windows.is_empty() {
                let dx = (x + SLOT / 2 - DOT / 2) * s;
                c.fill(dx, (PAD + SLOT + 1) * s, DOT * s, (DOT - 2) * s, [0.95, 0.96, 0.98, 1.0]);
            }
            if i > 0 && self.dock.items[i - 1].favorite && !item.favorite {
                c.fill(
                    (x - SEP / 2) * s,
                    (PAD + 8) * s,
                    s,
                    (SLOT - 16) * s,
                    [0.40, 0.44, 0.50, 1.0],
                );
            }
        }
        let buffer = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
            &c.rgba,
            smithay::backend::allocator::Fourcc::Abgr8888,
            (c.width, c.height),
            s,
            smithay::utils::Transform::Normal,
            None,
        );
        Some((buffer, (w, h)))
    }

    /// Load (or refresh) the installed apps' .desktop entries.
    pub fn ensure_entries(&mut self) {
        let stale = self.dock.entries_loaded.is_none_or(|t| t.elapsed().as_secs() > 120);
        if self.dock.entries.is_none() || stale {
            self.dock.entries = Some(crate::launcher::desktop::load_all());
            self.dock.entries_loaded = Some(Instant::now());
        }
    }

    /// An app's icon (premultiplied RGBA, `px` square) from its app_id, if it has one.
    pub fn app_icon(&mut self, app_id: &str, px: u32) -> Option<Vec<u8>> {
        self.ensure_entries();
        let icon =
            entry_for_app(self.dock.entries.as_deref().unwrap_or(&[]), app_id)?.icon.clone()?;
        self.dock
            .icons
            .entry(format!("{icon}@{px}"))
            .or_insert_with(|| crate::desktop::icons::load(&icon, px))
            .clone()
    }

    /// An icon by its name (the .desktop Icon= value), premultiplied RGBA, `px` square.
    pub fn icon_by_name(&mut self, icon: &str, px: u32) -> Option<Vec<u8>> {
        self.dock
            .icons
            .entry(format!("{icon}@{px}"))
            .or_insert_with(|| crate::desktop::icons::load(icon, px))
            .clone()
    }

    /// The name people know an app by ("Visual Studio Code" for "Code").
    pub fn app_display_name(&self, app_id: &str) -> String {
        self.dock
            .entries
            .as_deref()
            .and_then(|e| entry_for_app(e, app_id))
            .map_or_else(|| app_id.to_string(), |e| e.name.clone())
    }

    /// Mod went down / up while walking.
    pub fn set_dock_cursor(&mut self, on: bool) {
        if self.dock.cursor == on {
            return;
        }
        tracing::debug!(on, "dock cursor");
        self.dock.cursor = on;
        self.dock.cursor_moved = false;
        self.dock.hover = None;
        if on {
            // The cursor starts in the middle, where the crosshair was.
            if let Some(geo) = self.primary_output_geometry() {
                self.pointer_location = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
            }
            self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
        }
        self.dock.drawn = None;
        self.queue_redraw();
    }

    /// The dock cursor moved to `pos` (global logical).
    pub fn dock_pointer(&mut self, pos: Point<f64, Logical>) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let max: Point<f64, Logical> = Point::from((
            (geo.loc.x + geo.size.w) as f64 - 1.0,
            (geo.loc.y + geo.size.h) as f64 - 1.0,
        ));
        self.pointer_location = Point::from((
            pos.x.clamp(geo.loc.x as f64, max.x),
            pos.y.clamp(geo.loc.y as f64, max.y),
        ));
        self.dock.cursor_moved = true;
        let origin = dock_origin(&self.dock.items, (geo.size.w, geo.size.h));
        let local = self.pointer_location - geo.loc.to_f64();
        self.dock.hover =
            item_at(&self.dock.items, (local.x - origin.0 as f64, local.y - origin.1 as f64));
        self.queue_redraw();
    }

    /// A click with the dock cursor. Returns true if it hit an app.
    pub fn dock_click(&mut self, new_instance: bool) -> bool {
        let Some(item) = self.dock.hover.and_then(|h| self.dock.items.get(h).cloned()) else {
            return false;
        };
        let key = item.name.clone();
        if !new_instance && !item.windows.is_empty() {
            let n = self.dock.cycle.entry(key).or_insert(0);
            let panel = item.windows[*n % item.windows.len()];
            *n += 1;
            self.set_dock_cursor(false);
            self.summon_panel(panel);
            return true;
        }
        let Some(entry) = item.entry else { return true };
        let command = if entry.terminal {
            format!("{} -e {}", self.config.general.terminal, entry.exec)
        } else {
            entry.exec.clone()
        };
        tracing::info!(app = entry.name, "dock: launching");
        self.expect_window_until = Some(Instant::now() + crate::input::mode::SPAWN_FOCUS_WINDOW);
        self.set_dock_cursor(false);
        let _ = self.launch_named(&command, &entry.name);
        true
    }

    /// Bring a window in front of the view and use it.
    pub fn summon_panel(&mut self, panel: PanelId) {
        let camera = self.view.camera;
        let Some(size) = self.world.panels().iter().find(|p| p.id == panel).map(|p| p.size_px)
        else {
            return;
        };
        let d = self.spawn_distance(size);
        let forward = camera.forward_flat();
        if let Some(p) = self.world.panels_mut().iter_mut().find(|p| p.id == panel) {
            p.position = Vec3::new(
                camera.position.x + forward.x * d,
                camera.position.y,
                camera.position.z + forward.z * d,
            );
            p.face_towards(camera.position);
            p.visible = true;
        }
        tracing::info!(panel, "dock: window brought here");
        self.view.dirty = true;
        self.layout_changed();
        set_mode(self, InputMode::Window { panel }, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, exec: &str, wm: Option<&str>) -> DesktopEntry {
        DesktopEntry {
            id: format!("{id}.desktop"),
            name: name.into(),
            exec: exec.into(),
            terminal: false,
            icon: Some(id.into()),
            wm_class: wm.map(Into::into),
            terminal_emulator: false,
        }
    }

    fn entries() -> Vec<DesktopEntry> {
        vec![
            entry("firefox", "Firefox", "/usr/lib/firefox/firefox", None),
            entry("org.telegram.desktop", "Telegram", "Telegram --", None),
            entry("code", "Visual Studio Code", "/usr/bin/code", Some("Code")),
            entry("foot", "Foot", "foot", None),
        ]
    }

    #[test]
    fn specs_and_app_ids_find_their_apps() {
        let e = entries();
        assert_eq!(entry_for_spec(&e, "firefox").map(|e| e.name.as_str()), Some("Firefox"));
        assert_eq!(entry_for_spec(&e, "Telegram").map(|e| e.name.as_str()), Some("Telegram"));
        assert_eq!(entry_for_spec(&e, "code").map(|e| e.name.as_str()), Some("Visual Studio Code"));
        assert!(entry_for_spec(&e, "nautilus").is_none());
        assert_eq!(
            entry_for_app(&e, "org.telegram.desktop").map(|e| e.name.as_str()),
            Some("Telegram")
        );
        assert_eq!(entry_for_app(&e, "Code").map(|e| e.name.as_str()), Some("Visual Studio Code"));
        assert_eq!(entry_for_app(&e, "foot").map(|e| e.name.as_str()), Some("Foot"));
        assert!(entry_for_app(&e, "weston-terminal").is_none());
    }

    #[test]
    fn favorites_then_open_apps_grouped() {
        let e = entries();
        let favs = vec!["foot".to_string(), "firefox".to_string(), "missing".to_string()];
        let wins = vec![(1, "foot".to_string()), (2, "foot".to_string()), (3, "xeyes".to_string())];
        let items = build_items(&e, &favs, &wins);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Foot", "Firefox", "xeyes"]);
        assert_eq!(items[0].windows, vec![1, 2]);
        assert!(items[1].windows.is_empty());
        assert!(!items[2].favorite && items[2].entry.is_none());
    }

    #[test]
    fn hit_test_with_separator() {
        let e = entries();
        let items = build_items(&e, &["foot".to_string()], &[(9, "firefox".to_string())]);
        let (xs, w) = slots(&items);
        assert_eq!(xs, vec![PAD, PAD + SLOT + SEP]);
        assert_eq!(item_at(&items, ((PAD + 1) as f64, 10.0)), Some(0));
        assert_eq!(item_at(&items, ((PAD + SLOT + 2) as f64, 10.0)), None, "separator");
        assert_eq!(item_at(&items, ((w - PAD - 1) as f64, 10.0)), Some(1));
        assert_eq!(item_at(&items, (5.0, -1.0)), None);
    }
}
