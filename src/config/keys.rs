//! Keybinds: "Mod+Shift+Q"-style combos, the `[keybinds]` section and the lookup table.
//!
//! A combo is modifiers plus one key named like xkb does it ("Return", "q", "F5",
//! "equal", "KP_Add"; `xkbcli`/`wev` show the names). Keys are matched by the layout's
//! base symbol, so "Mod+Shift+Q" means the Q key with Shift held, on any layout.
//! "Mod" is the key chosen by `general.mod_key`.

use std::{collections::HashMap, fmt};

use serde::{de, Deserialize, Deserializer};
use smithay::input::keyboard::{xkb, Keysym, ModifiersState};

use super::{ModKey, Problem};
use crate::input::Action;

/// Modifiers of a combo. `alt` and `super_` mean those keys when they are NOT the Mod key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub mod_: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_: bool,
}

impl Mods {
    /// The modifiers held right now, in combo terms. Nested with mod_key = super, Alt
    /// alone also acts as Mod (the host desktop usually grabs Super combos).
    pub fn from_state(m: &ModifiersState, mod_key: ModKey, nested: bool) -> Self {
        match mod_key {
            ModKey::Super => {
                let alt_is_mod = nested && m.alt && !m.logo;
                Mods {
                    mod_: m.logo || alt_is_mod,
                    shift: m.shift,
                    ctrl: m.ctrl,
                    alt: m.alt && !alt_is_mod,
                    super_: false,
                }
            }
            ModKey::Alt => {
                Mods { mod_: m.alt, shift: m.shift, ctrl: m.ctrl, alt: false, super_: m.logo }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub mods: Mods,
    pub keysym: Keysym,
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.mods;
        for (on, name) in [
            (m.mod_, "Mod"),
            (m.super_, "Super"),
            (m.ctrl, "Ctrl"),
            (m.alt, "Alt"),
            (m.shift, "Shift"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&xkb::keysym_get_name(self.keysym))
    }
}

/// Key names people commonly mistype, for "did you mean" hints.
const COMMON_KEYS: &[&str] = &[
    "Return",
    "Escape",
    "space",
    "Tab",
    "BackSpace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "Prior",
    "Next",
    "Up",
    "Down",
    "Left",
    "Right",
    "plus",
    "minus",
    "equal",
    "comma",
    "period",
    "slash",
    "Print",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "KP_Add",
    "KP_Subtract",
    "KP_Enter",
];

/// Parse "Mod+Shift+Q" (case-insensitive).
pub fn parse_combo(s: &str) -> Result<KeyCombo, String> {
    if s.contains('\0') {
        return Err("key combo contains a NUL byte".into());
    }
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return Err("empty key combo".into());
    };
    if key.is_empty() {
        return Err(format!("'{s}': missing key after '+' (the + key is called 'plus')"));
    }
    let mut mods = Mods::default();
    for m in modifiers {
        let flag = match m.to_ascii_lowercase().as_str() {
            "mod" => &mut mods.mod_,
            "shift" => &mut mods.shift,
            "ctrl" | "control" => &mut mods.ctrl,
            "alt" => &mut mods.alt,
            "super" | "logo" | "win" | "meta" => &mut mods.super_,
            "" => return Err(format!("'{s}': empty modifier")),
            other => {
                return Err(format!(
                    "'{s}': unknown modifier '{other}' (use Mod, Shift, Ctrl, Alt, Super)"
                ))
            }
        };
        if *flag {
            return Err(format!("'{s}': modifier '{m}' given twice"));
        }
        *flag = true;
    }
    Ok(KeyCombo { mods, keysym: parse_key(key).map_err(|e| format!("'{s}': {e}"))? })
}

fn parse_key(name: &str) -> Result<Keysym, String> {
    // A single character ("q", "1", "=") is that character's key.
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        let sym = xkb::utf32_to_keysym(c.to_lowercase().next().unwrap_or(c) as u32);
        if sym.raw() != keysyms_no_symbol() {
            return Ok(sym);
        }
    }
    // Case-insensitive lookup prefers the lowercase symbol ("Q" -> q), which is what
    // the base layer of the key produces.
    let sym = xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE);
    if sym.raw() == keysyms_no_symbol() {
        let names: Vec<String> = COMMON_KEYS.iter().map(|s| s.to_string()).collect();
        return Err(match super::closest(name, &names) {
            Some(best) => format!("unknown key name '{name}' (did you mean '{best}'?)"),
            None => format!("unknown key name '{name}'"),
        });
    }
    Ok(sym)
}

fn keysyms_no_symbol() -> u32 {
    smithay::input::keyboard::keysyms::KEY_NoSymbol
}

/// One action's binding: a combo, a list of combos, or "" / [] for unbound.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Binding(pub Vec<String>);

impl Binding {
    fn one(s: &str) -> Self {
        Self(vec![s.into()])
    }
    fn many(s: &[&str]) -> Self {
        Self(s.iter().map(|s| s.to_string()).collect())
    }
}

impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = Binding;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(r#"a key combo like "Mod+Return", or a list of them"#)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Binding, E> {
                Ok(if v.trim().is_empty() { Binding(Vec::new()) } else { Binding(vec![v.into()]) })
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Binding, A::Error> {
                let mut combos = Vec::new();
                while let Some(s) = seq.next_element::<String>()? {
                    combos.push(s);
                }
                Ok(Binding(combos))
            }
        }
        d.deserialize_any(V)
    }
}

/// `[[keybinds.spawn]]`: a key that starts a program.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnBind {
    pub key: String,
    pub command: String,
}

/// The `[keybinds]` section: action name -> combo(s).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeybindsConfig {
    pub spawn_terminal: Binding,
    pub close: Binding,
    pub quit: Binding,
    /// Back to walking from a window. Mod+Escape always works as well, so a bad edit
    /// can never trap you inside a window.
    pub walk_mode: Binding,
    pub grab: Binding,
    pub rotate: Binding,
    pub reset_rotation: Binding,
    pub scale_up: Binding,
    pub scale_down: Binding,
    pub hide: Binding,
    pub debug: Binding,
    pub reload_shaders: Binding,
    /// Block types 1..8 for building, in order.
    pub select_block: Binding,
    pub spawn: Vec<SpawnBind>,
}

impl Default for KeybindsConfig {
    fn default() -> Self {
        Self {
            spawn_terminal: Binding::one("Mod+Return"),
            close: Binding::one("Mod+Shift+Q"),
            quit: Binding::one("Mod+Shift+E"),
            walk_mode: Binding::one("Mod+Escape"),
            grab: Binding::one("Mod+G"),
            rotate: Binding::one("Mod+R"),
            reset_rotation: Binding::one("Mod+Shift+R"),
            // "+" is Shift+"=" on most layouts.
            scale_up: Binding::many(&["Mod+equal", "Mod+Shift+equal", "Mod+KP_Add"]),
            scale_down: Binding::many(&["Mod+minus", "Mod+KP_Subtract"]),
            hide: Binding::one("Mod+H"),
            debug: Binding::one("Mod+F3"),
            reload_shaders: Binding::one("Mod+F5"),
            select_block: Binding::many(&[
                "Mod+1", "Mod+2", "Mod+3", "Mod+4", "Mod+5", "Mod+6", "Mod+7", "Mod+8",
            ]),
            spawn: Vec::new(),
        }
    }
}

impl KeybindsConfig {
    /// Every (config key, combo text, action), in file order.
    fn entries(&self) -> Vec<(String, &str, Action)> {
        let mut out = Vec::new();
        let singles = [
            ("spawn_terminal", &self.spawn_terminal, Action::SpawnTerminal),
            ("close", &self.close, Action::CloseWindow),
            ("quit", &self.quit, Action::Quit),
            ("walk_mode", &self.walk_mode, Action::WalkMode),
            ("grab", &self.grab, Action::Grab),
            ("rotate", &self.rotate, Action::RotatePanel),
            ("reset_rotation", &self.reset_rotation, Action::ResetPanelRotation),
            ("scale_up", &self.scale_up, Action::ScalePanelUp),
            ("scale_down", &self.scale_down, Action::ScalePanelDown),
            ("hide", &self.hide, Action::TogglePanelHidden),
            ("debug", &self.debug, Action::ToggleDebug),
            ("reload_shaders", &self.reload_shaders, Action::ReloadShaders),
        ];
        for (key, binding, action) in singles {
            for c in &binding.0 {
                out.push((format!("keybinds.{key}"), c.as_str(), action));
            }
        }
        for (i, c) in self.select_block.0.iter().enumerate() {
            out.push(("keybinds.select_block".into(), c.as_str(), Action::SelectBlock(i as u8)));
        }
        for (i, s) in self.spawn.iter().enumerate() {
            out.push((format!("keybinds.spawn[{i}].key"), s.key.as_str(), Action::Spawn(i)));
        }
        out
    }

    pub fn validate(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        if self.select_block.0.len() > crate::world::block::Block::PLACEABLE.len() {
            problems.push(super::problem(
                "keybinds.select_block",
                format!(
                    "at most {} keys (one per block type)",
                    crate::world::block::Block::PLACEABLE.len()
                ),
            ));
        }
        for (i, s) in self.spawn.iter().enumerate() {
            if let Err(e) = super::check_command(&s.command) {
                problems.push(super::problem(format!("keybinds.spawn[{i}].command"), e));
            }
        }
        let mut seen: HashMap<KeyCombo, String> = HashMap::new();
        for (key, text, _) in self.entries() {
            match parse_combo(text) {
                Ok(combo) => {
                    if let Some(other) = seen.get(&combo) {
                        problems.push(super::problem(
                            key.clone(),
                            format!("{combo} is already bound by {other}"),
                        ));
                    } else {
                        seen.insert(combo, key);
                    }
                }
                Err(e) => problems.push(super::problem(key, e)),
            }
        }
        problems
    }
}

/// The lookup table built from a validated config.
#[derive(Debug, Clone, Default)]
pub struct Keybinds {
    map: HashMap<KeyCombo, Action>,
}

impl Keybinds {
    pub fn new(config: &KeybindsConfig) -> Self {
        let map = config
            .entries()
            .into_iter()
            .filter_map(|(_, text, action)| Some((parse_combo(text).ok()?, action)))
            .collect();
        Self { map }
    }

    pub fn get(&self, mods: Mods, sym: Keysym) -> Option<Action> {
        self.map.get(&KeyCombo { mods, keysym: sym }).copied()
    }
}

/// Check that xkb can build a keymap from these names (a typo in a layout name would
/// otherwise leave you without a keyboard).
pub fn check_keymap(layout: &str, variant: &str, options: &str) -> Result<(), String> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let options = (!options.is_empty()).then(|| options.to_string());
    xkb::Keymap::new_from_names(
        &context,
        "",
        "",
        layout,
        variant,
        options,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .map(|_| ())
    .ok_or_else(|| format!("xkb cannot build layout '{layout}' (variant '{variant}')"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(mods: Mods, sym: Keysym) -> KeyCombo {
        KeyCombo { mods, keysym: sym }
    }
    const MOD: Mods = Mods { mod_: true, shift: false, ctrl: false, alt: false, super_: false };

    #[test]
    fn parses_valid_combos() {
        assert_eq!(parse_combo("Mod+Return"), Ok(combo(MOD, Keysym::Return)));
        let shift_q = Mods { shift: true, ..MOD };
        assert_eq!(parse_combo("Mod+Shift+Q"), Ok(combo(shift_q, Keysym::q)));
        assert_eq!(parse_combo("mod + shift + q"), Ok(combo(shift_q, Keysym::q)));
        assert_eq!(parse_combo("MOD+F5"), Ok(combo(MOD, Keysym::F5)));
        assert_eq!(parse_combo("Mod+="), Ok(combo(MOD, Keysym::equal)));
        assert_eq!(parse_combo("Mod+equal"), Ok(combo(MOD, Keysym::equal)));
        assert_eq!(parse_combo("Mod+1"), Ok(combo(MOD, Keysym::_1)));
        assert_eq!(parse_combo("Mod+kp_add"), Ok(combo(MOD, Keysym::KP_Add)));
        let ctrl_alt = Mods { ctrl: true, alt: true, ..Mods::default() };
        assert_eq!(parse_combo("Ctrl+Alt+Delete"), Ok(combo(ctrl_alt, Keysym::Delete)));
        assert_eq!(parse_combo("Print"), Ok(combo(Mods::default(), Keysym::Print)));
    }

    #[test]
    fn rejects_invalid_combos() {
        assert!(parse_combo("").is_err());
        assert!(parse_combo("Mod+").is_err());
        assert!(parse_combo("Mod++").is_err());
        assert!(parse_combo("Hyper+Q").unwrap_err().contains("unknown modifier"));
        assert!(parse_combo("Mod+Mod+Q").unwrap_err().contains("twice"));
        let e = parse_combo("Mod+Retrun").unwrap_err();
        assert!(e.contains("did you mean 'Return'"), "{e}");
        assert!(parse_combo("Mod+\0").is_err());
    }

    #[test]
    fn display_round_trips() {
        for s in ["Mod+Shift+q", "Mod+Return", "Ctrl+Alt+Delete", "Mod+F5"] {
            let c = parse_combo(s).expect("valid");
            assert_eq!(parse_combo(&c.to_string()), Ok(c), "{s} -> {c}");
        }
    }

    #[test]
    fn defaults_are_valid_and_unique() {
        let config = KeybindsConfig::default();
        assert_eq!(config.validate(), Vec::new());
        let binds = Keybinds::new(&config);
        assert_eq!(binds.get(MOD, Keysym::Return), Some(Action::SpawnTerminal));
        assert_eq!(binds.get(Mods { shift: true, ..MOD }, Keysym::q), Some(Action::CloseWindow));
        assert_eq!(binds.get(MOD, Keysym::q), None, "Mod+Q alone is not close");
        assert_eq!(
            binds.get(Mods { shift: true, ..MOD }, Keysym::equal),
            Some(Action::ScalePanelUp)
        );
        assert_eq!(binds.get(MOD, Keysym::_3), Some(Action::SelectBlock(2)));
    }

    #[test]
    fn duplicates_are_reported() {
        let config = KeybindsConfig { hide: Binding::one("Mod+G"), ..Default::default() };
        let problems = config.validate();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].message.contains("already bound by keybinds.grab"), "{problems:?}");
    }

    #[test]
    fn spawn_binds_map_to_their_index() {
        let config = KeybindsConfig {
            spawn: vec![SpawnBind { key: "Mod+B".into(), command: "firefox".into() }],
            ..Default::default()
        };
        assert_eq!(Keybinds::new(&config).get(MOD, Keysym::b), Some(Action::Spawn(0)));
    }

    #[test]
    fn event_modifiers() {
        let held = |shift, ctrl, alt, logo| ModifiersState {
            shift,
            ctrl,
            alt,
            logo,
            ..Default::default()
        };
        // Super is Mod.
        assert_eq!(Mods::from_state(&held(false, false, false, true), ModKey::Super, false), MOD);
        // Alt is just Alt on a real session...
        let alt = Mods::from_state(&held(false, false, true, false), ModKey::Super, false);
        assert_eq!(alt, Mods { alt: true, ..Mods::default() });
        // ...but Mod when nested.
        assert_eq!(Mods::from_state(&held(false, false, true, false), ModKey::Super, true), MOD);
        // mod_key = alt.
        assert_eq!(
            Mods::from_state(&held(true, false, true, false), ModKey::Alt, false),
            Mods { shift: true, ..MOD }
        );
    }

    #[test]
    fn layouts_are_checked() {
        assert!(check_keymap("us", "", "").is_ok());
        assert!(check_keymap("us,ru", "", "grp:alt_shift_toggle").is_ok());
        assert!(check_keymap("no_such_layout_xyz", "", "").is_err());
    }
}
