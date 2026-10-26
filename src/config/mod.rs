//! The config file: `$XDG_CONFIG_HOME/spacewm/config.toml` (usually ~/.config/spacewm/).
//!
//! Every key is optional and defaults to spacewm's built-in behavior, so an empty file is a
//! valid config. On first start a fully commented default file is written (from
//! `resources/config.toml`, which a unit test keeps in sync with `Config::default()`).
//!
//! Errors are meant for humans editing the file: they name the line, column and key,
//! show the offending line, and suggest the closest valid key for typos.
//! The file is watched (`watch.rs`) and changes are applied live (`apply.rs`); a broken
//! edit keeps the previous settings and shows the error on screen.

mod apply;
pub mod edit;
pub mod keys;
pub mod watch;

use std::{
    fmt,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{de, Deserialize, Deserializer};

/// The commented default config written on first start.
pub const DEFAULT_CONFIG: &str = include_str!("../../resources/config.toml");

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub input: Input,
    pub render: Render,
    pub panels: Panels,
    pub desktop: Desktop,
    pub dock: Dock,
    /// `[[output]]` tables: per-monitor mode and scale (DRM backend).
    #[serde(rename = "output")]
    pub outputs: Vec<OutputConfig>,
    pub keybinds: keys::KeybindsConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModKey {
    Super,
    Alt,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// The "Mod" in keybinds. Nested (winit) with "super", Alt also acts as Mod because
    /// the host desktop usually takes Super combos first.
    pub mod_key: ModKey,
    /// Started by the spawn_terminal keybind.
    pub terminal: String,
    /// Commands started once at startup (after --spawn ones).
    pub autostart: Vec<String>,
    pub world_seed: u64,
    /// Let `spacectl spawn --shell` run commands through `sh -c`. Off: any local process of
    /// yours could otherwise run arbitrary shell through spacewm.
    pub ipc_allow_shell: bool,
    /// Building with blocks: E inventory, hotbar, 1-5 keys, left click breaks and right
    /// click places blocks. Off: clicks never change the world by accident.
    pub building: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            mod_key: ModKey::Super,
            terminal: "foot".into(),
            autostart: Vec::new(),
            world_seed: crate::world::terrain::DEFAULT_SEED,
            ipc_allow_shell: false,
            building: false,
        }
    }
}

/// The dock along the bottom (hold Mod for a cursor to click it).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Dock {
    pub enabled: bool,
    /// Favorite apps, by .desktop file name, program or name; missing ones are skipped.
    pub apps: Vec<String>,
}

impl Default for Dock {
    fn default() -> Self {
        let apps = ["foot", "firefox", "org.telegram.desktop", "code", "org.gnome.Nautilus"];
        Self { enabled: true, apps: apps.iter().map(|s| s.to_string()).collect() }
    }
}

/// Everyday desktop things: locking, screenshots, media keys, restoring apps. Commands
/// are run without a shell (split like a shell would); "" switches a feature off.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Desktop {
    /// Started by the lock keybind and after `lock_after_minutes` without input.
    pub lock_command: String,
    /// Lock after this many minutes without input (not while a video plays). 0: never.
    pub lock_after_minutes: u32,
    /// Start the apps that were open when spacewm last exited.
    pub restore_apps: bool,
    /// Where Print saves screenshots ("~" is your home).
    pub screenshot_dir: String,
    /// Prints "x,y wxh" for Shift+Print (a region you drag).
    pub region_selector: String,
    /// On a real session without a notification daemon, start this one.
    pub notification_daemon: String,
    pub volume_up: String,
    pub volume_down: String,
    pub volume_mute: String,
    /// Prints the volume (a 0..1 number or a percentage; "MUTED" if muted).
    pub volume_get: String,
    pub mic_mute: String,
    pub mic_get: String,
    pub brightness_up: String,
    pub brightness_down: String,
    /// Prints the brightness (a percentage somewhere in its output).
    pub brightness_get: String,
    pub play_pause: String,
    pub next_track: String,
    pub previous_track: String,
}

impl Default for Desktop {
    fn default() -> Self {
        let s = |v: &str| v.to_string();
        Self {
            lock_command: s("swaylock -f -c 101418"),
            lock_after_minutes: 10,
            restore_apps: true,
            screenshot_dir: s("~/Pictures/Screenshots"),
            region_selector: s("slurp"),
            notification_daemon: s("mako"),
            volume_up: s("wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+"),
            volume_down: s("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-"),
            volume_mute: s("wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"),
            volume_get: s("wpctl get-volume @DEFAULT_AUDIO_SINK@"),
            mic_mute: s("wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle"),
            mic_get: s("wpctl get-volume @DEFAULT_AUDIO_SOURCE@"),
            brightness_up: s("brightnessctl -q set 5%+"),
            // -n: never all the way to 0 (a black screen you can't see to fix).
            brightness_down: s("brightnessctl -q -n set 5%-"),
            brightness_get: s("brightnessctl -m"),
            play_pause: s("playerctl play-pause"),
            next_track: s("playerctl next"),
            previous_track: s("playerctl previous"),
        }
    }
}

impl Desktop {
    /// (key, command) for every command setting, to validate them.
    fn commands(&self) -> [(&'static str, &str); 15] {
        [
            ("lock_command", &self.lock_command),
            ("region_selector", &self.region_selector),
            ("notification_daemon", &self.notification_daemon),
            ("volume_up", &self.volume_up),
            ("volume_down", &self.volume_down),
            ("volume_mute", &self.volume_mute),
            ("volume_get", &self.volume_get),
            ("mic_mute", &self.mic_mute),
            ("mic_get", &self.mic_get),
            ("brightness_up", &self.brightness_up),
            ("brightness_down", &self.brightness_down),
            ("brightness_get", &self.brightness_get),
            ("play_pause", &self.play_pause),
            ("next_track", &self.next_track),
            ("previous_track", &self.previous_track),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseAccel {
    /// Same hand movement, same turn: best for aiming in Walk mode.
    Flat,
    /// libinput's default: faster movements go proportionally further.
    Adaptive,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Input {
    /// Radians of camera turn per pixel of mouse movement.
    pub mouse_sensitivity: f32,
    pub invert_y: bool,
    /// Metres per second.
    pub walk_speed: f32,
    pub sprint_speed: f32,
    /// Panels farther than this (metres) can't be entered by clicking.
    pub reach_distance: f32,
    /// 1.0 = the cursor on a panel moves as fast as a normal desktop cursor.
    pub window_cursor_speed: f64,
    /// xkb layout list, e.g. "us,ru". Empty: $XKB_DEFAULT_LAYOUT, else "us".
    pub keyboard_layout: String,
    pub keyboard_variant: String,
    /// e.g. "grp:alt_shift_toggle,caps:escape".
    pub keyboard_options: String,
    /// Milliseconds before a held key repeats.
    pub repeat_delay: i32,
    /// Repeats per second (0: no repeat).
    pub repeat_rate: i32,
    pub touchpad_natural_scroll: bool,
    pub touchpad_tap: bool,
    pub mouse_accel: MouseAccel,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 0.0025,
            invert_y: false,
            walk_speed: crate::world::player::WALK_SPEED,
            sprint_speed: crate::world::player::SPRINT_SPEED,
            reach_distance: 6.0,
            window_cursor_speed: 1.0,
            keyboard_layout: String::new(),
            keyboard_variant: String::new(),
            keyboard_options: String::new(),
            repeat_delay: 200,
            repeat_rate: 25,
            touchpad_natural_scroll: true,
            touchpad_tap: true,
            mouse_accel: MouseAccel::Flat,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Render {
    /// Vertical field of view in degrees.
    pub fov: f32,
    /// How far blocks are drawn, in 16-block chunks.
    pub render_distance_chunks: i32,
    /// Fade far things into the sky.
    pub fog: bool,
    /// Smooth (mipmapped) panel textures: text stays readable at a distance.
    pub panel_mipmaps: bool,
    /// When nothing moves, redraws caused by apps (a blinking cursor, a video) are capped
    /// at this rate. 0: no cap (the monitor's refresh rate).
    pub max_fps_idle: u32,
    /// The clock/mode/title line in the top-left corner (Mod+F1 toggles it).
    pub hud: bool,
    /// Frames follow the monitor's refresh (DRM page flips, or the host's pace when
    /// nested), so there is never tearing. `false` (tearing for lower latency) is not
    /// supported yet.
    pub vsync: bool,
}

impl Default for Render {
    fn default() -> Self {
        Self {
            fov: 75.0,
            render_distance_chunks: 8,
            fog: true,
            panel_mipmaps: true,
            max_fps_idle: 0,
            hud: true,
            vsync: true,
        }
    }
}

/// How far in front of the camera a new panel appears.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpawnDistance {
    /// So that one window pixel is one screen pixel (sharp text), shrunk to fit the view.
    Auto,
    Metres(f32),
}

impl<'de> Deserialize<'de> for SpawnDistance {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = SpawnDistance;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(r#""auto" or a distance in metres"#)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v {
                    "auto" => Ok(SpawnDistance::Auto),
                    _ => Err(E::invalid_value(de::Unexpected::Str(v), &self)),
                }
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(SpawnDistance::Metres(v as f32))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(SpawnDistance::Metres(v as f32))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Panels {
    /// Window pixels per metre for new panels: 800 makes a 1000 px terminal 1.25 m wide.
    pub default_pixels_per_unit: f32,
    pub spawn_distance: SpawnDistance,
    /// In Window mode, other panels are drawn slightly transparent.
    pub dim_unfocused: bool,
    /// Entering a window glides the view in front of it (sharp text); leaving glides back.
    pub focus_zoom: bool,
    /// A carried window leans to face you: put it up high and it looks down at you.
    pub auto_tilt: bool,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            default_pixels_per_unit: crate::world::panel::DEFAULT_PIXELS_PER_UNIT,
            spawn_distance: SpawnDistance::Auto,
            dim_unfocused: true,
            focus_zoom: true,
            auto_tilt: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    /// Connector name as in `spacewm --drm-probe`, e.g. "HDMI-A-1".
    pub name: String,
    /// "1920x1080" or "1920x1080@144".
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub scale: Option<f64>,
}

/// A parsed output mode request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModeRequest {
    pub width: u16,
    pub height: u16,
    pub refresh: Option<f64>,
}

pub fn parse_mode(s: &str) -> Result<ModeRequest, String> {
    let (size, refresh) = match s.split_once('@') {
        Some((size, hz)) => {
            let hz: f64 = hz.trim().parse().map_err(|_| format!("bad refresh rate in '{s}'"))?;
            if !(hz.is_finite() && (1.0..=1000.0).contains(&hz)) {
                return Err(format!("refresh rate out of range in '{s}'"));
            }
            (size, Some(hz))
        }
        None => (s, None),
    };
    let (w, h) = crate::cli::parse_size(size.trim())?;
    Ok(ModeRequest { width: w as u16, height: h as u16, refresh })
}

impl OutputConfig {
    pub fn mode_request(&self) -> Option<ModeRequest> {
        self.mode.as_deref().and_then(|m| parse_mode(m).ok())
    }
}

#[cfg(test)]
impl Config {
    /// The output section for connector `name`, if any.
    pub fn output(&self, name: &str) -> Option<&OutputConfig> {
        self.outputs.iter().find(|o| o.name == name)
    }
}

// ---------------------------------------------------------------------------------------
// Loading and errors

/// One thing wrong with a config file, with where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    /// Dotted key path, e.g. "input.mouse_sensitivity" (empty for syntax errors).
    pub key: String,
    pub message: String,
    pub hint: Option<String>,
    /// 1-based line and column, 0 if unknown.
    pub line: usize,
    pub column: usize,
}

#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The file can't be used: every problem found, in file order.
    Invalid {
        path: PathBuf,
        source: String,
        problems: Vec<Problem>,
    },
}

impl std::error::Error for ConfigError {}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Invalid { path, source, problems } => {
                for p in problems {
                    if p.line > 0 {
                        write!(f, "{}:{}:{}: ", path.display(), p.line, p.column)?;
                    } else {
                        write!(f, "{}: ", path.display())?;
                    }
                    if p.key.is_empty() {
                        writeln!(f, "{}", p.message)?;
                    } else {
                        writeln!(f, "{}: {}", p.key, p.message)?;
                    }
                    if let Some(text) = p.line.checked_sub(1).and_then(|l| source.lines().nth(l)) {
                        writeln!(f, "    {text}")?;
                        writeln!(f, "    {}^", " ".repeat(p.column.saturating_sub(1)))?;
                    }
                    if let Some(hint) = &p.hint {
                        writeln!(f, "    hint: {hint}")?;
                    }
                }
                Ok(())
            }
        }
    }
}

impl ConfigError {
    /// A short version for the on-screen error panel.
    pub fn short(&self) -> Vec<String> {
        match self {
            Self::Io { path, error } => vec![format!("{}: {error}", path.display())],
            Self::Invalid { path, problems, .. } => {
                let mut lines = vec![format!("Config error in {}", path.display())];
                for p in problems.iter().take(6) {
                    let at = if p.line > 0 { format!("line {}: ", p.line) } else { String::new() };
                    let key = if p.key.is_empty() { String::new() } else { format!("{}: ", p.key) };
                    lines.push(format!("{at}{key}{}", p.message));
                    if let Some(hint) = &p.hint {
                        lines.push(format!("  {hint}"));
                    }
                }
                if problems.len() > 6 {
                    lines.push(format!("... and {} more", problems.len() - 6));
                }
                lines.push("The previous settings are still active.".into());
                lines
            }
        }
    }
}

/// `$XDG_CONFIG_HOME/spacewm/config.toml`, falling back to ~/.config/spacewm/config.toml.
pub fn default_path() -> anyhow::Result<PathBuf> {
    let dirs = directories::BaseDirs::new().context("cannot determine home directory")?;
    Ok(dirs.config_dir().join("spacewm").join("config.toml"))
}

/// Read and validate a config file.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| ConfigError::Io { path: path.into(), error })?;
    parse(&source).map_err(|problems| ConfigError::Invalid { path: path.into(), source, problems })
}

/// Parse config text: syntax and types first (serde), then values (`validate`).
pub fn parse(source: &str) -> Result<Config, Vec<Problem>> {
    let config: Config = toml::from_str(source).map_err(|e| vec![syntax_problem(source, &e)])?;
    let mut problems = config.validate();
    for p in &mut problems {
        if p.line == 0 {
            (p.line, p.column) = locate(source, &p.key);
        }
    }
    problems.sort_by_key(|p| (p.line == 0, p.line, p.column));
    if problems.is_empty() {
        Ok(config)
    } else {
        Err(problems)
    }
}

/// The config at `path`; a missing file is created with the commented defaults first.
/// Returns defaults plus the error if the file is broken (spacewm still starts).
pub fn load_or_create(path: &Path) -> (Config, Option<ConfigError>) {
    if !path.exists() {
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, DEFAULT_CONFIG));
        match written {
            Ok(()) => tracing::info!(path = %path.display(), "wrote default config"),
            Err(error) => {
                return (Config::default(), Some(ConfigError::Io { path: path.into(), error }));
            }
        }
    }
    match load(path) {
        Ok(config) => (config, None),
        Err(e) => (Config::default(), Some(e)),
    }
}

/// Turn a toml/serde error into a located problem with a "did you mean" hint.
fn syntax_problem(source: &str, error: &toml::de::Error) -> Problem {
    let (line, column) = error.span().map(|s| line_col(source, s.start)).unwrap_or((0, 0));
    let message = error.message().trim().to_string();
    // serde's messages for typos: "unknown field `x`, expected one of `a`, `b`" and
    // "unknown variant `x`, expected `a` or `b`". The full list is noise next to a good
    // suggestion, so it is only shown when there is none.
    let Some((name, expected)) = unknown_name(&message) else {
        return Problem { key: String::new(), message, hint: None, line, column };
    };
    let is_key = message.starts_with("unknown field");
    let hint = match closest(&name, &expected) {
        Some(best) if is_key => format!("unknown key '{name}', did you mean '{best}'?"),
        Some(best) => format!("did you mean '{best}'?"),
        None => format!("valid: {}", expected.join(", ")),
    };
    let (key, message) = if is_key {
        (name, "unknown key".to_string())
    } else {
        (String::new(), format!("unknown value '{name}'"))
    };
    Problem { key, message, hint: Some(hint), line, column }
}

/// From "unknown field `x`, expected one of `a`, `b`": ("x", ["a", "b"]).
fn unknown_name(message: &str) -> Option<(String, Vec<String>)> {
    let rest = message
        .strip_prefix("unknown field `")
        .or_else(|| message.strip_prefix("unknown variant `"))?;
    let (name, rest) = rest.split_once('`')?;
    let expected = rest.split('`').skip(1).step_by(2).map(str::to_string).collect();
    Some((name.to_string(), expected))
}

/// The candidate closest to `name` by edit distance, if it is close enough to be a typo.
pub fn closest<'a>(name: &str, candidates: &'a [String]) -> Option<&'a str> {
    let limit = (name.len() / 3).max(2);
    candidates
        .iter()
        .map(|c| (levenshtein(&name.to_lowercase(), &c.to_lowercase()), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c.as_str())
}

/// Edit distance: insertions, deletions and substitutions (one row of the DP table).
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let before = &source[..offset.min(source.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

/// Find where `section.key` (or `section[n].key` for `[[output]]`) is set in the file.
fn locate(source: &str, path: &str) -> (usize, usize) {
    let (section, key) = match path.rsplit_once('.') {
        Some((s, k)) => (s, k),
        None => ("", path),
    };
    let (table, index) = match section.split_once('[') {
        Some((t, i)) => (t, i.trim_end_matches(']').parse::<usize>().ok()),
        None => (section, None),
    };
    let mut current = String::new();
    let mut seen = 0usize;
    for (n, line) in source.lines().enumerate() {
        let t = line.trim();
        if let Some(name) = t.strip_prefix("[[").and_then(|r| r.split_once("]]")).map(|(n, _)| n) {
            current = name.trim().to_string();
            if current == table {
                seen += 1;
            }
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|r| r.split_once(']')).map(|(n, _)| n) {
            current = name.trim().to_string();
            continue;
        }
        let in_section = current == table && index.is_none_or(|i| seen == i + 1);
        if in_section {
            if let Some(rest) = t.strip_prefix(key) {
                if rest.trim_start().starts_with('=') {
                    let column = line.len() - line.trim_start().len() + 1;
                    return (n + 1, column);
                }
            }
        }
    }
    (0, 0)
}

// ---------------------------------------------------------------------------------------
// Value checks (types are already right: serde checked them)

fn check_range<T: PartialOrd + fmt::Display + Copy>(
    problems: &mut Vec<Problem>,
    key: &str,
    value: T,
    min: T,
    max: T,
) {
    // `!(a <= v && v <= b)` also catches NaN.
    if !(min <= value && value <= max) {
        problems.push(Problem {
            key: key.into(),
            message: format!("{value} is out of range ({min} to {max})"),
            hint: None,
            line: 0,
            column: 0,
        });
    }
}

fn problem(key: impl Into<String>, message: impl Into<String>) -> Problem {
    Problem { key: key.into(), message: message.into(), hint: None, line: 0, column: 0 }
}

impl Config {
    fn validate(&self) -> Vec<Problem> {
        let mut p = Vec::new();
        let i = &self.input;
        check_range(&mut p, "input.mouse_sensitivity", i.mouse_sensitivity, 0.00001, 0.1);
        check_range(&mut p, "input.walk_speed", i.walk_speed, 0.1, 50.0);
        check_range(&mut p, "input.sprint_speed", i.sprint_speed, 0.1, 50.0);
        check_range(&mut p, "input.reach_distance", i.reach_distance, 0.5, 50.0);
        check_range(&mut p, "input.window_cursor_speed", i.window_cursor_speed, 0.05, 20.0);
        check_range(&mut p, "input.repeat_delay", i.repeat_delay, 0, 5000);
        check_range(&mut p, "input.repeat_rate", i.repeat_rate, 0, 200);
        if let Err(e) =
            keys::check_keymap(&i.keyboard_layout, &i.keyboard_variant, &i.keyboard_options)
        {
            p.push(problem("input.keyboard_layout", e));
        }

        let r = &self.render;
        check_range(&mut p, "render.fov", r.fov, 30.0, 120.0);
        check_range(&mut p, "render.render_distance_chunks", r.render_distance_chunks, 2, 16);
        check_range(&mut p, "render.max_fps_idle", r.max_fps_idle, 0, 1000);

        let pn = &self.panels;
        check_range(
            &mut p,
            "panels.default_pixels_per_unit",
            pn.default_pixels_per_unit,
            200.0,
            3000.0,
        );
        if let SpawnDistance::Metres(m) = pn.spawn_distance {
            check_range(&mut p, "panels.spawn_distance", m, 0.2, 20.0);
        }

        for (n, o) in self.outputs.iter().enumerate() {
            if o.name.trim().is_empty() {
                p.push(problem(format!("output[{n}].name"), "must not be empty"));
            }
            if let Some(mode) = &o.mode {
                if let Err(e) = parse_mode(mode) {
                    p.push(problem(format!("output[{n}].mode"), e));
                }
            }
            if let Some(scale) = o.scale {
                check_range(&mut p, &format!("output[{n}].scale"), scale, 0.5, 4.0);
            }
        }

        for (n, cmd) in self.general.autostart.iter().enumerate() {
            if let Err(e) = check_command(cmd) {
                p.push(problem(format!("general.autostart[{n}]"), e));
            }
        }
        if let Err(e) = check_command(&self.general.terminal) {
            p.push(problem("general.terminal", e));
        }
        let d = &self.desktop;
        check_range(&mut p, "desktop.lock_after_minutes", d.lock_after_minutes, 0, 1440);
        for (key, cmd) in d.commands() {
            // "" switches the feature off.
            if !cmd.trim().is_empty() {
                if let Err(e) = check_command(cmd) {
                    p.push(problem(format!("desktop.{key}"), e));
                }
            }
        }
        p.extend(self.keybinds.validate());
        p
    }
}

fn check_command(cmd: &str) -> Result<(), String> {
    match shell_words::split(cmd) {
        Ok(words) if !words.is_empty() => Ok(()),
        Ok(_) => Err("empty command".into()),
        Err(e) => Err(format!("cannot split '{cmd}': {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_the_default() {
        assert_eq!(parse("").expect("empty config is valid"), Config::default());
    }

    #[test]
    fn shipped_default_file_matches_the_built_in_defaults() {
        let parsed = parse(DEFAULT_CONFIG).unwrap_or_else(|p| panic!("{p:?}"));
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn values_are_read() {
        let c = parse(
            "[general]\nmod_key = \"alt\"\n[input]\nmouse_sensitivity = 0.004\ninvert_y = true\n\
             [render]\nfov = 90\n[panels]\nspawn_distance = 1.5\n\
             [[output]]\nname = \"HDMI-A-1\"\nmode = \"1920x1080@144\"\nscale = 1.5\n",
        )
        .expect("valid");
        assert_eq!(c.general.mod_key, ModKey::Alt);
        assert_eq!(c.input.mouse_sensitivity, 0.004);
        assert!(c.input.invert_y);
        assert_eq!(c.render.fov, 90.0);
        assert_eq!(c.panels.spawn_distance, SpawnDistance::Metres(1.5));
        let o = c.output("HDMI-A-1").expect("output section");
        assert_eq!(
            o.mode_request(),
            Some(ModeRequest { width: 1920, height: 1080, refresh: Some(144.0) })
        );
        assert_eq!(o.scale, Some(1.5));
    }

    #[test]
    fn typo_gets_line_column_and_suggestion() {
        let src = "[input]\nwalk_speed = 4.0\n  mous_sensitivity = 0.01\n";
        let p = &parse(src).expect_err("typo")[0];
        assert_eq!((p.line, p.column), (3, 3));
        assert_eq!(p.key, "mous_sensitivity");
        assert_eq!(
            p.hint.as_deref(),
            Some("unknown key 'mous_sensitivity', did you mean 'mouse_sensitivity'?")
        );
        let shown = ConfigError::Invalid {
            path: "config.toml".into(),
            source: src.into(),
            problems: vec![p.clone()],
        }
        .to_string();
        assert!(shown.starts_with("config.toml:3:3:"), "{shown}");
        assert!(shown.contains("did you mean 'mouse_sensitivity'"), "{shown}");
    }

    #[test]
    fn unknown_section_is_suggested_too() {
        let p = &parse("[rendr]\nfov = 80\n").expect_err("typo")[0];
        assert_eq!(p.hint.as_deref(), Some("unknown key 'rendr', did you mean 'render'?"));
        assert_eq!(p.line, 1);
    }

    #[test]
    fn bad_enum_value_lists_the_choices() {
        let p = &parse("[general]\nmod_key = \"hyper\"\n").expect_err("bad value")[0];
        assert_eq!(p.line, 2);
        assert!(p.hint.as_deref().is_some_and(|h| h.contains("super")), "{p:?}");
    }

    #[test]
    fn wrong_type_is_reported_with_position() {
        let p = &parse("[render]\nfov = \"wide\"\n").expect_err("wrong type")[0];
        assert_eq!(p.line, 2);
        assert!(p.message.contains("invalid type"), "{p:?}");
    }

    #[test]
    fn out_of_range_values_are_located() {
        let src = "[render]\nfov = 170\n[input]\nwalk_speed = -1\n";
        let problems = parse(src).expect_err("ranges");
        assert_eq!(problems.len(), 2);
        assert_eq!((problems[0].key.as_str(), problems[0].line), ("render.fov", 2));
        assert_eq!((problems[1].key.as_str(), problems[1].line), ("input.walk_speed", 4));
    }

    #[test]
    fn nan_is_rejected() {
        assert!(parse("[render]\nfov = nan\n").is_err());
    }

    #[test]
    fn bad_output_mode() {
        let src = "[[output]]\nname = \"DP-1\"\n[[output]]\nname = \"DP-2\"\nmode = \"big\"\n";
        let p = &parse(src).expect_err("bad mode")[0];
        assert_eq!((p.key.as_str(), p.line), ("output[1].mode", 5));
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("same", "same"), 0);
        let names = vec!["mouse_sensitivity".to_string(), "invert_y".to_string()];
        assert_eq!(closest("mouse_sensitivty", &names), Some("mouse_sensitivity"));
        assert_eq!(closest("zzzz", &names), None);
    }
}
