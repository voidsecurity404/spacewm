//! A temporary message box drawn on top of everything (e.g. "config error, line 3").
//! Screen-space rather than a panel in the world, so it is visible wherever you look.

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

pub struct Notice {
    pub buffer: MemoryRenderBuffer,
    /// Size in logical px.
    pub size: (i32, i32),
    timer: Option<RegistrationToken>,
}

impl SpaceState {
    pub fn show_notice(&mut self, lines: Vec<String>, style: Style, secs: u64) {
        self.clear_notice();
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1);
        let Some(image) = text::render(&lines, style, scale) else { return };
        let timer = self
            .loop_handle
            .insert_source(Timer::from_duration(Duration::from_secs(secs)), |_, _, state| {
                if let Some(n) = state.notice.as_mut() {
                    n.timer = None;
                }
                state.clear_notice();
                TimeoutAction::Drop
            })
            .ok();
        self.notice = Some(Notice {
            buffer: image.buffer(),
            size: (image.width / image.scale, image.height / image.scale),
            timer,
        });
        self.queue_redraw();
    }

    pub fn clear_notice(&mut self) {
        if let Some(notice) = self.notice.take() {
            if let Some(token) = notice.timer {
                self.loop_handle.remove(token);
            }
            self.queue_redraw();
        }
    }
}
