//! Everyday desktop features, so spacewm works as the only desktop you use:
//!
//! - media keys (volume, mic, brightness, play/pause) with an on-screen display (OSD)
//!   showing the new level;
//! - screenshots (Print: whole screen, Shift+Print: a region) saved to a folder and put
//!   on the clipboard;
//! - locking (Mod+L) and locking by itself after some minutes without input (not while
//!   a video plays: players hold an idle inhibitor);
//! - a low battery warning, battery/date/keyboard layout in the HUD (status.rs);
//! - reopening the apps that were open last time (restore.rs);
//! - on a real session: starting a notification daemon if none runs.
//!
//! The tools (wpctl, brightnessctl, grim...) are run on helper threads: a slow or hung
//! tool must never freeze the desktop. Every command has a timeout; results come back to
//! the event loop through a calloop channel.

pub mod icons;
pub mod launching;
pub mod osd;
pub mod restore;
pub mod status;

use std::{
    ffi::OsString,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use smithay::reexports::calloop::{
    channel::{self, Sender},
    timer::{TimeoutAction, Timer},
    LoopHandle,
};

use crate::state::SpaceState;

/// A media key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hw {
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MicMute,
    BrightnessUp,
    BrightnessDown,
    PlayPause,
    NextTrack,
    PreviousTrack,
}

/// What a helper thread reports back.
#[derive(Debug)]
pub enum Msg {
    /// Show a level on the OSD.
    Level { what: osd::Kind, percent: Option<u32>, muted: bool },
    /// Show a message (info or error).
    Note { text: String, error: bool },
    /// The volume, read in the background for the 2D bar.
    Volume { percent: Option<u32>, muted: bool },
    /// A program spacewm started has exited (`None`: killed by a signal).
    Exited { pid: u32, code: Option<i32> },
}

/// Commands that take longer than this are killed.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
/// How often idle (auto-lock) and the battery are looked at.
const TICK: Duration = Duration::from_secs(15);
/// Warn once when the battery drops to this, unplugged.
const LOW_BATTERY: u32 = 10;

#[derive(Default)]
pub struct DesktopState {
    tx: Option<Sender<Msg>>,
    pub osd: Option<osd::Osd>,
    /// When the user last pressed a key, moved or clicked.
    pub last_input: Option<Instant>,
    /// Auto-lock already ran since the last input (don't start lockers in a loop if the
    /// command fails).
    auto_locked: bool,
    low_battery_warned: bool,
    /// Last battery reading for the HUD, refreshed every TICK.
    pub battery: Option<status::Battery>,
    /// Apps on their way ("Starting Firefox ...").
    pub launching: launching::Launching,
    /// For the 2D bar: volume (percent, muted) and Wi-Fi signal.
    pub volume: Option<(u32, bool)>,
    pub wifi: Option<u8>,
}

/// Set up the result channel and the periodic tick. Called once at startup.
pub fn init(state: &mut SpaceState, handle: &LoopHandle<'static, SpaceState>) {
    let (tx, rx) = channel::channel::<Msg>();
    let registered = handle.insert_source(rx, |event, _, state| {
        if let channel::Event::Msg(msg) = event {
            state.desktop_msg(msg);
        }
    });
    if let Err(e) = registered {
        tracing::warn!("desktop helper channel: {}", e.error);
        return;
    }
    crate::spawn::set_exit_sender(tx.clone());
    state.desktop.tx = Some(tx);
    state.desktop.last_input = Some(Instant::now());
    state.desktop.battery = status::battery();
    state.desktop.wifi = status::wifi();
    state.read_volume();
    let _ = handle.insert_source(Timer::from_duration(TICK), |_, _, state| {
        state.desktop_tick();
        TimeoutAction::ToDuration(TICK)
    });
}

/// Run a command line (no shell) with spacewm's session environment, wait at most
/// COMMAND_TIMEOUT, return its stdout (None if it failed to start, timed out or exited
/// with an error).
fn run(cmd: &str, socket: &OsString, stdin: Option<std::fs::File>) -> Option<String> {
    run_with(cmd, socket, stdin, true)
}

/// `run`, for tools that leave a process behind (wl-copy keeps serving the clipboard):
/// their output is not collected, since reading it would wait for that process too.
fn run_quiet(cmd: &str, socket: &OsString, stdin: Option<std::fs::File>) -> bool {
    run_with(cmd, socket, stdin, false).is_some()
}

fn run_with(
    cmd: &str,
    socket: &OsString,
    stdin: Option<std::fs::File>,
    capture: bool,
) -> Option<String> {
    let args = shell_words::split(cmd).ok()?;
    let (program, rest) = args.split_first()?;
    let mut command = crate::spawn::session_command(program, socket);
    command
        .args(rest)
        .stdin(stdin.map_or_else(Stdio::null, Stdio::from))
        .stdout(if capture { Stdio::piped() } else { Stdio::null() })
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|e| tracing::debug!("{program}: {e}")).ok()?;
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                tracing::warn!(command = cmd, "command timed out; killed");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    status.success().then_some(out)
}

/// "~/x" -> "$HOME/x".
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

impl SpaceState {
    /// Any user input (see `input_activity`).
    pub fn desktop_input(&mut self) {
        self.desktop.last_input = Some(Instant::now());
        self.desktop.auto_locked = false;
    }

    fn send_to_thread(&self) -> Option<(Sender<Msg>, OsString)> {
        Some((self.desktop.tx.clone()?, self.socket_name.clone()))
    }

    /// A media key: run the tool on a helper thread, then show the new level.
    pub fn hardware_key(&mut self, hw: Hw) {
        let d = &self.config.desktop;
        let (action, get, what) = match hw {
            Hw::VolumeUp => (&d.volume_up, &d.volume_get, Some(osd::Kind::Volume)),
            Hw::VolumeDown => (&d.volume_down, &d.volume_get, Some(osd::Kind::Volume)),
            Hw::VolumeMute => (&d.volume_mute, &d.volume_get, Some(osd::Kind::Volume)),
            Hw::MicMute => (&d.mic_mute, &d.mic_get, Some(osd::Kind::Microphone)),
            Hw::BrightnessUp => (&d.brightness_up, &d.brightness_get, Some(osd::Kind::Brightness)),
            Hw::BrightnessDown => {
                (&d.brightness_down, &d.brightness_get, Some(osd::Kind::Brightness))
            }
            Hw::PlayPause => (&d.play_pause, &String::new(), None),
            Hw::NextTrack => (&d.next_track, &String::new(), None),
            Hw::PreviousTrack => (&d.previous_track, &String::new(), None),
        };
        if action.trim().is_empty() {
            return;
        }
        let (action, get) = (action.clone(), get.clone());
        let Some((tx, socket)) = self.send_to_thread() else { return };
        tracing::debug!(?hw, "media key");
        std::thread::spawn(move || {
            let ok = run(&action, &socket, None).is_some();
            let Some(what) = what else {
                if !ok {
                    let _ = tx.send(Msg::Note { text: format!("failed: {action}"), error: true });
                }
                return;
            };
            let reading = (!get.trim().is_empty()).then(|| run(&get, &socket, None)).flatten();
            let msg = match reading {
                Some(out) => Msg::Level {
                    what,
                    percent: status::parse_percent(&out),
                    muted: status::parse_muted(&out),
                },
                None if ok => Msg::Level { what, percent: None, muted: false },
                None => Msg::Note { text: format!("failed: {action}"), error: true },
            };
            let _ = tx.send(msg);
        });
    }

    /// Print / Shift+Print.
    pub fn screenshot(&mut self, region: bool) {
        if self.lock.is_locked() {
            return;
        }
        let dir = expand_home(&self.config.desktop.screenshot_dir);
        let selector = self.config.desktop.region_selector.clone();
        let Some((tx, socket)) = self.send_to_thread() else { return };
        let (y, mo, d, _, h, mi, s) = status::local_now();
        let file = dir.join(format!("spacewm-{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}.png"));
        std::thread::spawn(move || {
            let note = |text: String, error: bool| {
                let _ = tx.send(Msg::Note { text, error });
            };
            let geometry = if region {
                match run(&selector, &socket, None).map(|g| g.trim().to_string()) {
                    Some(g) if !g.is_empty() => Some(g),
                    // Escape in the selector: no screenshot, no message.
                    _ => return,
                }
            } else {
                None
            };
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return note(format!("cannot create {}: {e}", dir.display()), true);
            }
            let path = file.to_string_lossy().into_owned();
            let grim = match &geometry {
                Some(g) => shell_words::join(["grim", "-g", g.as_str(), path.as_str()]),
                None => shell_words::join(["grim", path.as_str()]),
            };
            if run(&grim, &socket, None).is_none() {
                return note("screenshot failed (is grim installed?)".into(), true);
            }
            // The clipboard too, so it can be pasted right away. wl-copy keeps serving
            // it in the background; its own process exits at once.
            let copied = std::fs::File::open(&file)
                .ok()
                .is_some_and(|f| run_quiet("wl-copy --type image/png", &socket, Some(f)));
            tracing::info!(file = %file.display(), copied, "screenshot saved");
            let home = std::env::var("HOME").unwrap_or_default();
            let shown = match path.strip_prefix(&home) {
                Some(rest) if !home.is_empty() => format!("~{rest}"),
                _ => path.clone(),
            };
            let tail = if copied { " (copied)" } else { "" };
            note(format!("Screenshot saved: {shown}{tail}"), false);
        });
    }

    /// Mod+L, and auto-lock.
    pub fn lock_now(&mut self) {
        if self.lock.is_locked() {
            return;
        }
        let cmd = self.config.desktop.lock_command.clone();
        if cmd.trim().is_empty() {
            self.show_error(vec!["No lock_command set in [desktop]".into()]);
            return;
        }
        tracing::info!(command = cmd, "locking");
        if let Err(e) = self.launch_untracked(&cmd) {
            self.show_error(vec![format!("Locking failed: {e}")]);
        }
    }

    fn desktop_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Level { what, percent, muted } => {
                if what == osd::Kind::Volume {
                    self.desktop.volume = percent.map(|p| (p, muted));
                }
                self.show_osd(what, percent, muted)
            }
            Msg::Volume { percent, muted } => {
                let v = percent.map(|p| (p, muted));
                if v != self.desktop.volume {
                    self.desktop.volume = v;
                    self.queue_redraw();
                }
            }
            Msg::Note { text, error: true } => self.show_error(vec![text]),
            Msg::Note { text, error: false } => self.show_info(text),
            Msg::Exited { pid, code } => self.launch_exited(pid, code),
        }
    }

    /// A short blue message (like the one for separately started apps).
    pub fn show_info(&mut self, text: String) {
        let style = crate::render::text::Style {
            size: 16.0,
            fg: [1.0, 1.0, 1.0, 1.0],
            bg: [0.10, 0.22, 0.42, 0.93],
            padding: 12,
            max_width: 900,
        };
        self.show_notice(vec![text], style, 4);
    }

    /// Every TICK: auto-lock, battery.
    fn desktop_tick(&mut self) {
        let minutes = self.config.desktop.lock_after_minutes;
        let idle = self.desktop.last_input.map_or(Duration::ZERO, |t| t.elapsed());
        self.update_idle_inhibit();
        let inhibited = !self.idle_inhibitors.is_empty();
        // Nested, the desktop around us locks the screen; spacewm sees no input while you
        // use other windows there and would lock for nothing.
        if minutes > 0
            && !self.backend.is_nested()
            && !inhibited
            && !self.desktop.auto_locked
            && !self.lock.is_locked()
            && idle >= Duration::from_secs(u64::from(minutes) * 60)
        {
            tracing::info!(idle_s = idle.as_secs(), "no input for a while: auto-lock");
            self.desktop.auto_locked = true;
            self.lock_now();
        }

        let wifi = status::wifi();
        if wifi != self.desktop.wifi {
            self.desktop.wifi = wifi;
            self.queue_redraw();
        }
        self.read_volume();
        let battery = status::battery();
        if battery != self.desktop.battery {
            self.desktop.battery = battery;
            self.queue_redraw(); // the HUD shows it
        }
        match battery {
            Some(b)
                if !b.charging && b.percent <= LOW_BATTERY && !self.desktop.low_battery_warned =>
            {
                self.desktop.low_battery_warned = true;
                tracing::warn!(percent = b.percent, "battery low");
                self.show_error(vec![format!("Battery low: {}% - plug in the charger", b.percent)]);
            }
            Some(b) if b.charging || b.percent > LOW_BATTERY + 5 => {
                self.desktop.low_battery_warned = false;
            }
            _ => {}
        }
    }

    /// Read the volume in the background (for the 2D bar).
    fn read_volume(&mut self) {
        let get = self.config.desktop.volume_get.clone();
        if get.trim().is_empty() {
            return;
        }
        let Some((tx, socket)) = self.send_to_thread() else { return };
        std::thread::spawn(move || {
            if let Some(out) = run(&get, &socket, None) {
                let _ = tx.send(Msg::Volume {
                    percent: status::parse_percent(&out),
                    muted: status::parse_muted(&out),
                });
            }
        });
    }

    /// A real session has no desktop around it: start a notification daemon if nobody
    /// owns org.freedesktop.Notifications (apps' notify-send would go nowhere).
    pub fn start_notification_daemon(&mut self) {
        let cmd = self.config.desktop.notification_daemon.clone();
        if cmd.trim().is_empty() {
            return;
        }
        let Some((tx, socket)) = self.send_to_thread() else { return };
        std::thread::spawn(move || {
            // Give autostarted daemons a moment to claim the name first.
            std::thread::sleep(Duration::from_secs(3));
            let owned = run(
                "busctl --user --timeout=2 status org.freedesktop.Notifications",
                &socket,
                None,
            )
            .is_some();
            if owned {
                return;
            }
            let args = shell_words::split(&cmd).unwrap_or_default();
            let Some((program, rest)) = args.split_first() else { return };
            let mut command: Command = crate::spawn::session_command(program, &socket);
            command.args(rest).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            match command.spawn() {
                Ok(child) => {
                    tracing::info!(command = cmd, pid = child.id(), "notification daemon started");
                    // Reap it when it exits.
                    let mut child = child;
                    let _ = child.wait();
                }
                Err(e) => {
                    let _ = tx.send(Msg::Note { text: format!("{cmd}: {e}"), error: true });
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_time_out_and_report_failure() {
        let socket = OsString::from("wayland-test");
        assert_eq!(run("echo 0.42", &socket, None).as_deref().map(str::trim), Some("0.42"));
        assert_eq!(run("false", &socket, None), None);
        assert_eq!(run("spacewm-no-such-tool", &socket, None), None);
        let start = Instant::now();
        assert_eq!(run("sleep 30", &socket, None), None);
        assert!(start.elapsed() < COMMAND_TIMEOUT + Duration::from_secs(2));
    }

    #[test]
    fn home_is_expanded() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(expand_home("~/Pictures"), PathBuf::from(&home).join("Pictures"));
        assert_eq!(expand_home("/tmp/x"), PathBuf::from("/tmp/x"));
    }
}
