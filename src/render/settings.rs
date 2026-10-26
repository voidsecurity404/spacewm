//! The settings box (Escape while walking, or Mod+comma): the world (map, time of day,
//! shadows) and the settings people change most, changed with a click,
//! saved to the config file (only that line changes, comments stay) and applied at
//! once. Everything else is still in the file.
//!
//! Mouse: click a row for the next value, right click for the previous one. Keys: Up and
//! Down pick a row, Left and Right change it. Escape (or a click outside) closes it.

use smithay::{
    input::{keyboard::Keysym, pointer::CursorImageStatus},
    utils::{Logical, Point},
};
use toml_edit::Value;

use super::text::{self, Line, Style};
use crate::{
    config::Config,
    inventory::{draw::Canvas, Image},
    state::SpaceState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    MouseSpeed,
    InvertY,
    Fov,
    Map,
    TimeOfDay,
    Shadows,
    Fog,
    RenderDistance,
    Hud,
    DimUnfocused,
    FocusZoom,
    AutoTilt,
    CarryNew,
    LockAfter,
    RestoreApps,
    Dock,
    Layouts,
    LayoutSwitch,
    Terminal,
    Building,
}

pub const SETTINGS: [Setting; 20] = [
    Setting::MouseSpeed,
    Setting::InvertY,
    Setting::Fov,
    Setting::Map,
    Setting::TimeOfDay,
    Setting::Shadows,
    Setting::Fog,
    Setting::RenderDistance,
    Setting::Hud,
    Setting::DimUnfocused,
    Setting::FocusZoom,
    Setting::AutoTilt,
    Setting::CarryNew,
    Setting::LockAfter,
    Setting::RestoreApps,
    Setting::Dock,
    Setting::Layouts,
    Setting::LayoutSwitch,
    Setting::Terminal,
    Setting::Building,
];

const DEFAULT_SENSITIVITY: f64 = 0.0025;
const SENSITIVITIES: [f64; 9] = [0.001, 0.0015, 0.002, 0.0025, 0.003, 0.004, 0.005, 0.0065, 0.008];
const FOVS: [f64; 11] = [60.0, 65.0, 70.0, 75.0, 80.0, 85.0, 90.0, 95.0, 100.0, 105.0, 110.0];
const DISTANCES: [i64; 6] = [4, 6, 8, 10, 12, 16];
const LOCK_MINUTES: [i64; 8] = [0, 1, 2, 5, 10, 15, 30, 60];
const LAYOUTS: [&str; 6] = ["", "us", "us,ru", "us,uz", "us,ru,uz", "us,tr"];
const SWITCHES: [(&str, &str); 5] = [
    ("", "none"),
    ("grp:alt_shift_toggle", "Alt+Shift"),
    ("grp:win_space_toggle", "Super+Space"),
    ("grp:caps_toggle", "Caps Lock"),
    ("grp:ctrl_shift_toggle", "Ctrl+Shift"),
];
const TIMES: [&str; 3] = ["day", "afternoon", "evening"];
const TERMINALS: [&str; 7] =
    ["foot", "alacritty", "kitty", "ghostty", "wezterm", "konsole", "xterm"];

/// The entry after (dir 1) or before (-1) `current` in `list`; numbers between entries
/// snap to the nearest one first. `wrap`: past the end starts over (choices), else stop
/// (amounts).
fn step<T: Clone + PartialEq>(list: &[T], current: usize, dir: i32, wrap: bool) -> T {
    let n = list.len() as i32;
    let i = current as i32 + dir;
    let i = if wrap { i.rem_euclid(n) } else { i.clamp(0, n - 1) };
    list[i as usize].clone()
}

fn nearest(list: &[f64], v: f64) -> usize {
    list.iter()
        .enumerate()
        .min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs()))
        .map_or(0, |(i, _)| i)
}

fn installed(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

impl Setting {
    pub fn label(self) -> &'static str {
        match self {
            Setting::MouseSpeed => "Mouse speed",
            Setting::InvertY => "Invert mouse up/down",
            Setting::Fov => "Field of view",
            Setting::Map => "World",
            Setting::TimeOfDay => "Time of day",
            Setting::Shadows => "Sun shadows",
            Setting::Fog => "Fog in the distance",
            Setting::RenderDistance => "View distance",
            Setting::Hud => "Clock line (HUD)",
            Setting::DimUnfocused => "Dim other windows",
            Setting::FocusZoom => "Zoom into windows",
            Setting::AutoTilt => "Carried windows face me",
            Setting::CarryNew => "New windows in my hand",
            Setting::LockAfter => "Lock screen after",
            Setting::RestoreApps => "Reopen apps at start",
            Setting::Dock => "Dock",
            Setting::Layouts => "Keyboard layouts",
            Setting::LayoutSwitch => "Switch layout with",
            Setting::Terminal => "Terminal",
            Setting::Building => "Building blocks",
        }
    }

    /// What the row shows.
    pub fn value(self, c: &Config) -> String {
        let yes = |b: bool| if b { "on" } else { "off" }.to_string();
        match self {
            Setting::MouseSpeed => {
                format!(
                    "{:.0}%",
                    f64::from(c.input.mouse_sensitivity) / DEFAULT_SENSITIVITY * 100.0
                )
            }
            Setting::InvertY => yes(c.input.invert_y),
            Setting::Fov => format!("{:.0} deg", c.render.fov),
            Setting::Map => match c.world.map {
                crate::config::WorldMap::Dust => "Desert town".into(),
                crate::config::WorldMap::Voxel => "Blocks".into(),
            },
            Setting::TimeOfDay => {
                let t = c.world.time_of_day.name();
                let mut s = t.to_string();
                s[..1].make_ascii_uppercase();
                s
            }
            Setting::Shadows => yes(c.world.shadows),
            Setting::Fog => yes(c.render.fog),
            Setting::RenderDistance => format!("{} chunks", c.render.render_distance_chunks),
            Setting::Hud => yes(c.render.hud),
            Setting::DimUnfocused => yes(c.panels.dim_unfocused),
            Setting::FocusZoom => yes(c.panels.focus_zoom),
            Setting::AutoTilt => yes(c.panels.auto_tilt),
            Setting::CarryNew => yes(c.panels.carry_new_windows),
            Setting::LockAfter => match c.desktop.lock_after_minutes {
                0 => "never".into(),
                m => format!("{m} min"),
            },
            Setting::RestoreApps => yes(c.desktop.restore_apps),
            Setting::Dock => yes(c.dock.enabled),
            Setting::Layouts => match c.input.keyboard_layout.as_str() {
                "" => "system default".into(),
                l => l.to_string(),
            },
            Setting::LayoutSwitch => SWITCHES
                .iter()
                .find(|(o, _)| *o == c.input.keyboard_options)
                .map_or_else(|| c.input.keyboard_options.clone(), |(_, name)| name.to_string()),
            Setting::Terminal => c.general.terminal.clone(),
            Setting::Building => yes(c.general.building),
        }
    }

    /// The (table, key, new value) for one step in `dir` from the current config.
    pub fn changed(self, c: &Config, dir: i32) -> (&'static str, &'static str, Value) {
        let flip = |b: bool| Value::from(!b);
        let pos = |list: &[&str], v: &str| list.iter().position(|x| *x == v);
        match self {
            Setting::MouseSpeed => {
                let i = nearest(&SENSITIVITIES, f64::from(c.input.mouse_sensitivity));
                ("input", "mouse_sensitivity", Value::from(step(&SENSITIVITIES, i, dir, false)))
            }
            Setting::InvertY => ("input", "invert_y", flip(c.input.invert_y)),
            Setting::Fov => {
                let i = nearest(&FOVS, c.render.fov as f64);
                ("render", "fov", Value::from(step(&FOVS, i, dir, false)))
            }
            Setting::Map => {
                let next = match c.world.map {
                    crate::config::WorldMap::Dust => "voxel",
                    crate::config::WorldMap::Voxel => "dust",
                };
                ("world", "map", Value::from(next))
            }
            Setting::TimeOfDay => {
                let i = TIMES.iter().position(|t| *t == c.world.time_of_day.name()).unwrap_or(0);
                ("world", "time_of_day", Value::from(step(&TIMES, i, dir, true)))
            }
            Setting::Shadows => ("world", "shadows", flip(c.world.shadows)),
            Setting::Fog => ("render", "fog", flip(c.render.fog)),
            Setting::RenderDistance => {
                let f: Vec<f64> = DISTANCES.iter().map(|&d| d as f64).collect();
                let i = nearest(&f, c.render.render_distance_chunks as f64);
                ("render", "render_distance_chunks", Value::from(step(&DISTANCES, i, dir, false)))
            }
            Setting::Hud => ("render", "hud", flip(c.render.hud)),
            Setting::DimUnfocused => ("panels", "dim_unfocused", flip(c.panels.dim_unfocused)),
            Setting::FocusZoom => ("panels", "focus_zoom", flip(c.panels.focus_zoom)),
            Setting::AutoTilt => ("panels", "auto_tilt", flip(c.panels.auto_tilt)),
            Setting::CarryNew => ("panels", "carry_new_windows", flip(c.panels.carry_new_windows)),
            Setting::LockAfter => {
                let f: Vec<f64> = LOCK_MINUTES.iter().map(|&m| m as f64).collect();
                let i = nearest(&f, c.desktop.lock_after_minutes as f64);
                ("desktop", "lock_after_minutes", Value::from(step(&LOCK_MINUTES, i, dir, false)))
            }
            Setting::RestoreApps => ("desktop", "restore_apps", flip(c.desktop.restore_apps)),
            Setting::Dock => ("dock", "enabled", flip(c.dock.enabled)),
            Setting::Layouts => {
                let i = pos(&LAYOUTS, &c.input.keyboard_layout).unwrap_or(0);
                ("input", "keyboard_layout", Value::from(step(&LAYOUTS, i, dir, true)))
            }
            Setting::LayoutSwitch => {
                let opts: Vec<&str> = SWITCHES.iter().map(|(o, _)| *o).collect();
                let i = pos(&opts, &c.input.keyboard_options).unwrap_or(0);
                ("input", "keyboard_options", Value::from(step(&opts, i, dir, true)))
            }
            Setting::Terminal => {
                let have: Vec<&str> = TERMINALS.iter().copied().filter(|t| installed(t)).collect();
                let list = if have.is_empty() { vec!["foot"] } else { have };
                let i = pos(&list, &c.general.terminal).unwrap_or(0);
                ("general", "terminal", Value::from(step(&list, i, dir, true)))
            }
            Setting::Building => ("general", "building", flip(c.general.building)),
        }
    }
}

/// Layout in logical px.
const WIDTH: i32 = 470;
const PAD: i32 = 16;
const TITLE_H: i32 = 34;
const ROW_H: i32 = 30;
const FOOT_H: i32 = 46;

pub fn box_size() -> (i32, i32) {
    (WIDTH, PAD + TITLE_H + SETTINGS.len() as i32 * ROW_H + FOOT_H + PAD)
}

/// The row at a point relative to the box's top-left corner.
pub fn row_at(p: (f64, f64)) -> Option<usize> {
    let y0 = (PAD + TITLE_H) as f64;
    if p.0 < 0.0 || p.0 >= WIDTH as f64 || p.1 < y0 {
        return None;
    }
    let i = ((p.1 - y0) / ROW_H as f64) as usize;
    (i < SETTINGS.len()).then_some(i)
}

pub fn box_origin(output: (i32, i32)) -> (i32, i32) {
    let (w, h) = box_size();
    ((output.0 - w) / 2, ((output.1 - h) / 2).max(0))
}

pub struct SettingsBox {
    pub hover: Option<usize>,
    pub selected: usize,
    pub image: Option<Image>,
    drawn: Option<(Vec<String>, Option<usize>, usize, i32)>,
}

fn draw(
    values: &[String],
    hover: Option<usize>,
    selected: usize,
    file: &str,
    scale: i32,
) -> Option<Image> {
    let s = scale;
    let (w, h) = box_size();
    let mut c = Canvas::new(w * s, h * s);
    c.fill(0, 0, w * s, h * s, [0.07, 0.08, 0.10, 0.96]);
    c.frame(0, 0, w * s, h * s, s, [0.32, 0.36, 0.42, 1.0]);
    let base = Style {
        size: 15.0,
        fg: [0.95, 0.96, 0.98, 1.0],
        bg: [0.0; 4],
        padding: 0,
        max_width: 4000,
    };
    let line = |t: &str, fg: [f32; 4], size: f32| {
        text::render_lines(
            &[Line { text: t.into(), fg: Some(fg), bg: None }],
            Style { size, ..base },
            s,
            0,
        )
    };
    let title = line("Settings", [1.0, 0.86, 0.45, 1.0], 18.0)?;
    c.blit(&title.rgba, title.width, title.height, PAD * s, (PAD - 2) * s);
    for (i, setting) in SETTINGS.iter().enumerate() {
        let y = PAD + TITLE_H + i as i32 * ROW_H;
        if hover == Some(i) || (hover.is_none() && selected == i) {
            c.fill(
                (PAD - 6) * s,
                y * s,
                (w - 2 * PAD + 12) * s,
                ROW_H * s,
                [0.20, 0.40, 0.70, 1.0],
            );
        }
        let label = line(setting.label(), [0.92, 0.93, 0.95, 1.0], 15.0)?;
        c.blit(&label.rgba, label.width, label.height, PAD * s, (y + 6) * s);
        let v = format!("< {} >", values.get(i).map(String::as_str).unwrap_or(""));
        let value = line(&v, [0.60, 0.80, 1.0, 1.0], 15.0)?;
        c.blit(&value.rgba, value.width, value.height, (w - PAD) * s - value.width, (y + 6) * s);
    }
    // Two short lines; a long path keeps its end (the file name).
    let fy = PAD + TITLE_H + SETTINGS.len() as i32 * ROW_H + 8;
    let gray = [0.55, 0.58, 0.62, 1.0];
    let hint = line("click or arrows: change    Esc: close", gray, 12.0)?;
    c.blit(&hint.rgba, hint.width, hint.height, PAD * s, fy * s);
    let n = file.chars().count();
    let shown: String = if n > 52 {
        std::iter::once('…').chain(file.chars().skip(n - 51)).collect()
    } else {
        file.to_string()
    };
    let saved = line(&format!("saved to {shown}"), gray, 12.0)?;
    c.blit(&saved.rgba, saved.width, saved.height, PAD * s, (fy + 18) * s);
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

impl SpaceState {
    pub fn toggle_settings(&mut self) {
        if self.settings.is_some() {
            self.close_settings();
            return;
        }
        let Some(geo) = self.primary_output_geometry() else { return };
        tracing::info!("settings opened");
        self.close_menu();
        self.close_inventory();
        self.set_dock_cursor(false);
        self.view.controls.clear();
        self.pointer_location = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
        self.cursor_status = CursorImageStatus::default_named();
        self.settings = Some(SettingsBox { hover: None, selected: 0, image: None, drawn: None });
        self.view.dirty = true;
        self.queue_redraw();
    }

    pub fn close_settings(&mut self) {
        if self.settings.take().is_some() {
            tracing::info!("settings closed");
            self.view.dirty = true;
            self.queue_redraw();
        }
    }

    /// Change setting `i` one step (`dir` 1 or -1): write the file, apply it now.
    fn change_setting(&mut self, i: usize, dir: i32) {
        let Some(&setting) = SETTINGS.get(i) else { return };
        let (table, key, value) = setting.changed(&self.config, dir);
        tracing::info!(setting = format!("{table}.{key}"), %value, "setting changed");
        match crate::config::edit::set_value(&self.config_path, table, key, value) {
            // The watcher would apply it too, a moment later; now is nicer.
            Ok(()) => {
                let _ = self.reload_config();
            }
            Err(e) => self.show_error(vec![format!("{}: {e}", setting.label())]),
        }
        self.queue_redraw();
    }

    pub fn settings_pointer(&mut self, pos: Point<f64, Logical>) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let max: Point<f64, Logical> = Point::from((
            (geo.loc.x + geo.size.w) as f64 - 1.0,
            (geo.loc.y + geo.size.h) as f64 - 1.0,
        ));
        self.pointer_location = Point::from((
            pos.x.clamp(geo.loc.x as f64, max.x),
            pos.y.clamp(geo.loc.y as f64, max.y),
        ));
        let o = box_origin((geo.size.w, geo.size.h));
        let local =
            self.pointer_location - geo.loc.to_f64() - Point::from((o.0 as f64, o.1 as f64));
        if let Some(b) = self.settings.as_mut() {
            b.hover = row_at((local.x, local.y));
        }
        self.queue_redraw();
    }

    pub fn settings_click(&mut self, back: bool) {
        match self.settings.as_ref().map(|b| b.hover) {
            Some(Some(i)) => {
                if let Some(b) = self.settings.as_mut() {
                    b.selected = i;
                }
                self.change_setting(i, if back { -1 } else { 1 });
            }
            Some(None) => self.close_settings(),
            None => {}
        }
    }

    pub fn settings_key(&mut self, sym: Keysym) {
        let n = SETTINGS.len();
        let Some(sel) = self.settings.as_ref().map(|b| b.selected) else { return };
        match sym {
            Keysym::Escape => self.close_settings(),
            Keysym::Up | Keysym::Down => {
                if let Some(b) = self.settings.as_mut() {
                    b.hover = None;
                    b.selected =
                        if sym == Keysym::Down { (sel + 1) % n } else { (sel + n - 1) % n };
                }
                self.queue_redraw();
            }
            Keysym::Right | Keysym::Return | Keysym::KP_Enter => self.change_setting(sel, 1),
            Keysym::Left => self.change_setting(sel, -1),
            _ => {}
        }
    }

    /// Before each frame: redraw when values or the highlight changed.
    pub fn refresh_settings(&mut self) {
        let Some(b) = self.settings.as_ref() else { return };
        let values: Vec<String> = SETTINGS.iter().map(|s| s.value(&self.config)).collect();
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        let key = (values, b.hover, b.selected, scale);
        if b.drawn.as_ref() == Some(&key) {
            return;
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let path = self.config_path.display().to_string();
        let file = match path.strip_prefix(&home) {
            Some(rest) if !home.is_empty() => format!("~{rest}"),
            _ => path,
        };
        let image = draw(&key.0, key.1, key.2, &file, scale);
        if let Some(b) = self.settings.as_mut() {
            b.image = image;
            b.drawn = Some(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_steps_to_a_valid_config() {
        let mut c = Config::default();
        for s in SETTINGS {
            for dir in [1, -1] {
                let (table, key, value) = s.changed(&c, dir);
                let text = crate::config::edit::set_in_text(
                    crate::config::DEFAULT_CONFIG,
                    table,
                    key,
                    value,
                )
                .unwrap_or_else(|e| panic!("{s:?} {dir}: {e}"));
                c = crate::config::parse(&text).expect("valid");
            }
        }
    }

    #[test]
    fn steps() {
        let c = Config::default();
        let (_, _, v) = Setting::Fov.changed(&c, 1);
        assert_eq!(v.as_float(), Some(80.0));
        let (_, _, v) = Setting::LockAfter.changed(&c, -1);
        assert_eq!(v.as_integer(), Some(5));
        // Amounts stop at the ends, choices wrap around.
        assert_eq!(step(&[1, 2, 3], 2, 1, false), 3);
        assert_eq!(step(&[1, 2, 3], 2, 1, true), 1);
        assert_eq!(Setting::LockAfter.value(&c), "10 min");
        assert_eq!(Setting::MouseSpeed.value(&c), "100%");
    }

    #[test]
    fn rows_hit_test() {
        let y = (PAD + TITLE_H + 5) as f64;
        assert_eq!(row_at((10.0, y)), Some(0));
        assert_eq!(row_at((10.0, y + ROW_H as f64)), Some(1));
        assert_eq!(row_at((10.0, 5.0)), None);
        assert_eq!(row_at((-1.0, y)), None);
        let (_, h) = box_size();
        assert_eq!(row_at((10.0, (h - 2) as f64)), None);
    }
}
