//! The HUD: a small text line in the top-left corner with the clock, the input mode and
//! the focused window's title. Toggled with Mod+F1 (config `render.hud`).
//!
//! It is re-rendered only when its text changes; a timer wakes us at the start of every
//! minute for the clock, so an idle desktop still renders nothing in between.

use std::time::Duration;

use smithay::{
    backend::renderer::element::memory::MemoryRenderBuffer,
    reexports::calloop::{
        timer::{TimeoutAction, Timer},
        RegistrationToken,
    },
};

use super::text::{self, Style};
use crate::state::SpaceState;

#[derive(Default)]
pub struct Hud {
    pub visible: bool,
    text: String,
    /// See `current_layout_tag`.
    layout_tag: Option<String>,
    /// The rendered line and its size in logical px.
    pub image: Option<(MemoryRenderBuffer, (i32, i32))>,
    clock_timer: Option<RegistrationToken>,
}

/// Local wall-clock time as (hours, minutes, seconds).
fn local_time() -> (u32, u32, u32) {
    // SAFETY: time(NULL) and localtime_r with a zeroed out-struct are plain libc calls.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return (0, 0, 0);
        }
        (tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32)
    }
}

impl SpaceState {
    /// What the HUD should say right now.
    fn hud_text(&self) -> String {
        let (h, m, _) = local_time();
        let mode = if self.launcher.is_some() {
            "Launcher"
        } else if self.inventory.is_some() {
            "Inventory"
        } else if self.overlay_layer().is_some() {
            "Overlay"
        } else if self.render_mode == crate::cli::RenderMode::Flat2D {
            "2D"
        } else {
            match self.mode {
                crate::input::mode::InputMode::Walk if self.seated.active => "Seated",
                crate::input::mode::InputMode::Walk => "Walk",
                crate::input::mode::InputMode::Window { .. } => "Window",
            }
        };
        let focused = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let title = match focused {
            Some(crate::focus::KeyboardFocusTarget::Window(w)) => {
                let (app, title) = crate::state::window_names(&w);
                title.filter(|t| !t.is_empty()).or(app).unwrap_or_default()
            }
            _ => String::new(),
        };
        let (_, month, day, weekday, _, _, _) = crate::desktop::status::local_now();
        let mut s = format!(
            "{h:02}:{m:02}  {}   {mode}",
            crate::desktop::status::short_date(weekday, day, month)
        );
        if let Some(b) = self.desktop.battery {
            s.push_str(&format!("   Bat {}%{}", b.percent, if b.charging { "+" } else { "" }));
        }
        if let Some(tag) = &self.hud.layout_tag {
            s.push_str("   ");
            s.push_str(tag);
        }
        if !title.is_empty() {
            let short: String = title.chars().take(60).collect();
            s.push_str("   ");
            s.push_str(&short);
            if title.chars().count() > 60 {
                s.push('…');
            }
        }
        s
    }

    /// Called before every frame: re-render the HUD if its text changed.
    /// The active keyboard layout's tag ("EN"), only when there is more than one.
    fn current_layout_tag(&mut self) -> Option<String> {
        let keyboard = self.seat.get_keyboard()?;
        keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().ok()?;
            if xkb.layouts().count() < 2 {
                return None;
            }
            Some(crate::desktop::status::layout_tag(xkb.layout_name(xkb.active_layout())))
        })
    }

    pub fn refresh_hud(&mut self) {
        self.hud.layout_tag = self.current_layout_tag();
        // Nothing over a fullscreen window.
        if !self.hud.visible || self.fullscreen.is_some() {
            self.hud.image = None;
            return;
        }
        let text = self.hud_text();
        if self.hud.image.is_some() && text == self.hud.text {
            return;
        }
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1);
        let style = Style {
            size: 15.0,
            fg: [0.95, 0.96, 0.98, 1.0],
            bg: [0.05, 0.06, 0.08, 0.62],
            padding: 7,
            max_width: 1000,
        };
        self.hud.image = text::render(std::slice::from_ref(&text), style, scale)
            .map(|img| (img.buffer(), (img.width / img.scale, img.height / img.scale)));
        tracing::debug!(text, "hud");
        self.hud.text = text;
    }

    pub fn set_hud_visible(&mut self, visible: bool) {
        self.hud.visible = visible;
        if visible {
            self.schedule_clock_tick();
        } else if let Some(token) = self.hud.clock_timer.take() {
            self.loop_handle.remove(token);
        }
        self.hud.image = None;
        self.queue_redraw();
    }

    /// Wake up at the start of the next minute so the clock changes on time.
    fn schedule_clock_tick(&mut self) {
        if self.hud.clock_timer.is_some() {
            return;
        }
        let (_, _, sec) = local_time();
        let wait = Duration::from_secs(60 - u64::from(sec.min(59))) + Duration::from_millis(50);
        self.hud.clock_timer = self
            .loop_handle
            .insert_source(Timer::from_duration(wait), |_, _, state| {
                state.hud.clock_timer = None;
                if state.hud.visible {
                    state.queue_redraw();
                    state.schedule_clock_tick();
                }
                TimeoutAction::Drop
            })
            .ok();
    }
}
