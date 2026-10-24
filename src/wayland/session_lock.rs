//! ext-session-lock: screen lockers (swaylock).
//!
//! While locked, every output shows ONLY the locker's surface — no world, no panels, no
//! overlays — and all input goes to it. Security rules, because a locker is only as good
//! as the compositor enforcing it:
//!
//! - The lock is confirmed to the client (`locked`) only after a frame without any other
//!   content was rendered, so nothing private is still on screen when the locker thinks
//!   it is in charge.
//! - If the locker crashes, the session STAYS locked: outputs turn red with a "locked"
//!   message and only a new locker can unlock. Unlocking on a crash would mean killing
//!   swaylock (any process of yours can) opens the desktop.
//! - While a live locker holds the lock, another lock request is refused.
//! - Keyboard focus can't leave the lock surface (`set_keyboard_focus` is the only way
//!   to change focus and refuses), screen capture and IPC screenshots are refused.

use smithay::{
    backend::renderer::element::memory::MemoryRenderBuffer,
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
        wayland_server::{protocol::wl_output::WlOutput, Resource},
    },
    utils::SERIAL_COUNTER,
    wayland::session_lock::{
        LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
    },
};

use crate::{focus::KeyboardFocusTarget, state::SpaceState};

#[derive(Default)]
pub struct LockState {
    locked: Option<Locked>,
    /// "Locked" message for the red screen shown when the locker is gone.
    pub message: Option<(MemoryRenderBuffer, (i32, i32))>,
}

struct Locked {
    lock: ExtSessionLockV1,
    /// Confirmed to the client after the first locked frame.
    pending: Option<SessionLocker>,
    surfaces: Vec<(Output, LockSurface)>,
}

impl LockState {
    pub fn is_locked(&self) -> bool {
        self.locked.is_some()
    }

    /// The locker died without unlocking.
    pub fn is_abandoned(&self) -> bool {
        self.locked.as_ref().is_some_and(|l| !l.lock.is_alive())
    }

    /// The lock surface to show on `output`, if the locker made one.
    pub fn surface_for(&self, output: &Output) -> Option<&LockSurface> {
        let locked = self.locked.as_ref()?;
        locked.surfaces.iter().find(|(o, s)| o == output && s.alive()).map(|(_, s)| s)
    }

    fn first_surface(&self) -> Option<&LockSurface> {
        self.locked.as_ref()?.surfaces.iter().map(|(_, s)| s).find(|s| s.alive())
    }
}

impl SessionLockHandler for SpaceState {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if let Some(current) = &self.lock.locked {
            if current.lock.is_alive() {
                tracing::warn!("lock request refused: the session is already locked");
                return; // dropping the locker tells the client it failed
            }
            tracing::info!("new locker replaces the one that died");
        } else {
            tracing::info!("session locked");
        }
        self.lock.locked = Some(Locked {
            lock: confirmation.ext_session_lock().clone(),
            pending: Some(confirmation),
            surfaces: Vec::new(),
        });
        // Nothing but the locker from now on.
        self.close_launcher();
        self.close_inventory();
        self.close_help();
        self.overlay = crate::state::Overlay::None;
        self.view.controls.clear();
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
        }
        if self.lock.message.is_none() {
            let style = crate::render::text::Style {
                size: 28.0,
                fg: [1.0, 1.0, 1.0, 1.0],
                bg: [0.0, 0.0, 0.0, 0.0],
                padding: 0,
                max_width: 1200,
            };
            let lines = vec!["locked".to_string()];
            self.lock.message = crate::render::text::render(&lines, style, 1)
                .map(|img| (img.buffer(), (img.width, img.height)));
        }
        self.queue_redraw();
    }

    fn unlock(&mut self) {
        tracing::info!("session unlocked");
        self.lock.locked = None;
        self.restore_keyboard_focus();
        self.view.dirty = true;
        self.queue_redraw();
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        let Some(locked) = self.lock.locked.as_mut() else { return };
        if surface.ext_session_lock() != &locked.lock {
            return; // from a locker we refused
        }
        let Some(output) = Output::from_resource(&wl_output) else { return };
        let size = output
            .current_mode()
            .map(|m| m.size.to_f64().to_logical(output.current_scale().fractional_scale()))
            .map(|s| (s.w.round() as u32, s.h.round() as u32));
        surface.with_pending_state(|s| s.size = size.map(Into::into));
        surface.send_configure();
        locked.surfaces.retain(|(o, s)| o != &output && s.alive());
        locked.surfaces.push((output, surface.clone()));
        let focus = self.lock.first_surface().map(|s| s.wl_surface().clone());
        if let (Some(keyboard), Some(focus)) = (self.seat.get_keyboard(), focus) {
            keyboard.set_focus(
                self,
                Some(KeyboardFocusTarget::Surface(focus)),
                SERIAL_COUNTER.next_serial(),
            );
        }
        self.queue_redraw();
    }
}

impl SpaceState {
    /// After a frame was rendered: a lock waiting for confirmation is now visible.
    pub fn confirm_lock(&mut self) {
        if let Some(locker) = self.lock.locked.as_mut().and_then(|l| l.pending.take()) {
            locker.lock();
            tracing::info!("lock confirmed to the locker");
        }
    }

    /// THE way to move keyboard focus. While locked only the lock surface may have it.
    pub fn set_keyboard_focus(&mut self, target: Option<KeyboardFocusTarget>) {
        if self.lock.is_locked() {
            let allowed = matches!(&target, Some(KeyboardFocusTarget::Surface(s))
                if self.lock.first_surface().is_some_and(|l| l.wl_surface() == s));
            if !allowed {
                return;
            }
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
        }
    }
}
