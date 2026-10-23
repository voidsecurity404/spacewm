//! Applying a (re)loaded config to the running compositor.

use smithay::input::keyboard::XkbConfig;

use super::{keys::Keybinds, Config};
use crate::{input::mode::InputSettings, state::SpaceState};

/// How long the red error panel stays on screen.
const ERROR_NOTICE_SECS: u64 = 10;

impl SpaceState {
    /// The file changed (or `spacectl reload-config`): apply it, or keep the old settings and
    /// show what is wrong.
    pub fn reload_config(&mut self) -> Result<(), String> {
        match super::load(&self.config_path) {
            Ok(new) => {
                self.apply_config(new, false);
                self.clear_notice();
                tracing::info!(path = %self.config_path.display(), "config reloaded");
                crate::ipc::broadcast(self, crate::ipc::proto::Event::ConfigReloaded);
                Ok(())
            }
            Err(e) => {
                tracing::error!("config not applied, keeping the previous settings:\n{e}");
                self.show_error(e.short());
                Err(e.to_string())
            }
        }
    }

    /// Make the running compositor match `new`. `initial`: at startup (nothing to diff).
    pub fn apply_config(&mut self, new: Config, initial: bool) {
        let old = std::mem::replace(&mut self.config, new.clone());
        let new = &new;
        if !initial {
            let changed: Vec<&str> = [
                ("general", old.general != new.general),
                ("input", old.input != new.input),
                ("render", old.render != new.render),
                ("panels", old.panels != new.panels),
                ("output", old.outputs != new.outputs),
                ("keybinds", old.keybinds != new.keybinds),
            ]
            .into_iter()
            .filter_map(|(name, c)| c.then_some(name))
            .collect();
            tracing::info!(changed = changed.join(","), "applying config");
            // Things that only take effect when spacewm starts.
            let restart = [
                ("general.world_seed", old.general.world_seed != new.general.world_seed),
                ("general.autostart", old.general.autostart != new.general.autostart),
                ("output", old.outputs != new.outputs),
                ("render.vsync", old.render.vsync != new.render.vsync),
            ];
            for (key, changed) in restart {
                if changed {
                    tracing::warn!(key, "changed: needs restart");
                }
            }
        }

        // Input.
        self.input_settings = InputSettings::from_config(&new.input);
        self.player.sprint_speed = new.input.sprint_speed;
        self.backend.apply_input_settings(&self.input_settings);
        // At startup the keyboard was already created with this layout (state.rs).
        let keyboard_changed = !initial
            && (old.input.keyboard_layout != new.input.keyboard_layout
                || old.input.keyboard_variant != new.input.keyboard_variant
                || old.input.keyboard_options != new.input.keyboard_options);
        if let Some(keyboard) = self.seat.get_keyboard() {
            if keyboard_changed {
                let i = &new.input;
                let xkb = XkbConfig {
                    layout: &i.keyboard_layout,
                    variant: &i.keyboard_variant,
                    options: (!i.keyboard_options.is_empty()).then(|| i.keyboard_options.clone()),
                    ..XkbConfig::default()
                };
                let (layout, options) = (&i.keyboard_layout, &i.keyboard_options);
                match keyboard.set_xkb_config(self, xkb) {
                    Ok(()) => tracing::info!(layout, options, "keyboard layout"),
                    Err(e) => tracing::error!(layout, "keyboard layout not applied: {e:?}"),
                }
            }
            keyboard.change_repeat_info(new.input.repeat_rate, new.input.repeat_delay);
        }
        self.keybinds = Keybinds::new(&new.keybinds);
        self.world.default_ppu = new.panels.default_pixels_per_unit;

        // Rendering.
        if !new.render.vsync && (initial || old.render.vsync) {
            tracing::warn!("render.vsync = false is not supported yet; frames stay in sync");
        }
        let r = &self.config.render;
        self.view.camera.fov_y_deg = r.fov;
        self.view.settings = crate::render::view::RenderSettings::from_config(&self.config);
        if let Some(textures) = self.view.panel_textures.as_mut() {
            // Textures are recreated with or without their mip chain.
            if textures.mipmaps != r.panel_mipmaps {
                textures.set_mipmaps(r.panel_mipmaps);
                for p in self.world.panels_mut() {
                    p.content_dirty = true;
                }
            }
        }
        self.view.dirty = true;
        self.queue_redraw();
    }

    /// Show a red error panel in front of everything for a few seconds.
    pub fn show_error(&mut self, lines: Vec<String>) {
        let style = crate::render::text::Style {
            size: 17.0,
            fg: [1.0, 1.0, 1.0, 1.0],
            bg: [0.62, 0.08, 0.08, 0.93],
            padding: 14,
            max_width: 900,
        };
        self.show_notice(lines, style, ERROR_NOTICE_SECS);
    }
}
