//! IPC protocol: newline-delimited JSON, one request → one response.
//!
//! Shared by the server (spacewm) and the client (spacectl, which includes this file with
//! `#[path]`), so both always agree on the format. See docs/ipc.md.

use serde::{Deserialize, Serialize};

/// Largest accepted request line, in bytes.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Version,
    ListPanels,
    FocusPanel {
        id: u64,
    },
    ClosePanel {
        id: u64,
    },
    Spawn {
        command: String,
    },
    Screenshot {
        path: String,
        #[serde(default)]
        force: bool,
    },
    Quit,
    /// Move the camera (metres); optionally set where it looks (degrees).
    Teleport {
        position: [f32; 3],
        #[serde(default)]
        yaw_deg: Option<f32>,
        #[serde(default)]
        pitch_deg: Option<f32>,
    },
    /// Turn this connection into an event stream.
    Subscribe {
        events: Vec<EventKind>,
    },
    /// Synthetic input; only with `spacewm --allow-input-injection`.
    Input {
        action: InputAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PressMode {
    Press,
    Release,
    #[default]
    Tap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputAction {
    /// "Alt+Return", "ctrl+shift+c", "Escape".
    Key {
        combo: String,
        #[serde(default)]
        state: PressMode,
    },
    Type {
        text: String,
    },
    /// Absolute pointer position, output-local logical pixels.
    MoveTo {
        x: f64,
        y: f64,
    },
    /// Relative pointer motion.
    MoveBy {
        dx: f64,
        dy: f64,
    },
    Button {
        button: MouseButton,
        #[serde(default)]
        state: PressMode,
    },
    /// Wheel notches; positive dy scrolls down.
    Scroll {
        dx: f64,
        dy: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    PanelOpened,
    PanelClosed,
    FocusChanged,
    ModeChanged,
    ConfigReloaded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    PanelOpened { id: u64, app_id: Option<String> },
    PanelClosed { id: u64 },
    FocusChanged { id: Option<u64> },
    ModeChanged { mode: String },
    ConfigReloaded,
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Self::PanelOpened { .. } => EventKind::PanelOpened,
            Self::PanelClosed { .. } => EventKind::PanelClosed,
            Self::FocusChanged { .. } => EventKind::FocusChanged,
            Self::ModeChanged { .. } => EventKind::ModeChanged,
            Self::ConfigReloaded => EventKind::ConfigReloaded,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelInfo {
    pub id: u64,
    pub app_id: Option<String>,
    pub title: Option<String>,
    /// World position in metres (2D mode: output pixels, z = 0).
    pub position: [f64; 3],
    /// Panel rotation around Y in degrees; its front faces (sin yaw, 0, cos yaw).
    #[serde(default)]
    pub yaw_deg: f64,
    pub size_px: [i32; 2],
    pub focused: bool,
    /// "wayland" or "x11".
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Error {
        message: String,
    },
    Version {
        version: String,
        backend: String,
    },
    Panels {
        panels: Vec<PanelInfo>,
    },
    Spawned {
        pid: u32,
    },
    Screenshot {
        path: String,
        width: u32,
        height: u32,
    },
    Event {
        event: Event,
    },
}

impl Response {
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let requests = vec![
            Request::Version,
            Request::ListPanels,
            Request::Spawn {
                command: "foot -e htop".into(),
            },
            Request::Screenshot {
                path: "/home/u/a.png".into(),
                force: true,
            },
            Request::Subscribe {
                events: vec![EventKind::PanelOpened],
            },
            Request::Input {
                action: InputAction::Key {
                    combo: "Alt+Return".into(),
                    state: PressMode::Tap,
                },
            },
            Request::Input {
                action: InputAction::MoveTo { x: 1.5, y: 2.0 },
            },
        ];
        for r in requests {
            let line = serde_json::to_string(&r).expect("serialize");
            let back: Request = serde_json::from_str(&line).expect("deserialize");
            assert_eq!(r, back, "{line}");
        }
    }

    #[test]
    fn wire_format_is_stable() {
        let r: Request =
            serde_json::from_str(r#"{"request":"focus_panel","id":3}"#).expect("parse");
        assert_eq!(r, Request::FocusPanel { id: 3 });
        let r: Request =
            serde_json::from_str(r#"{"request":"input","action":{"action":"type","text":"hi"}}"#)
                .expect("parse");
        assert_eq!(
            r,
            Request::Input {
                action: InputAction::Type { text: "hi".into() }
            }
        );
        assert_eq!(
            serde_json::to_string(&Response::Ok).expect("serialize"),
            r#"{"response":"ok"}"#
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // (serde can't deny extra fields on unit variants like `quit`; they carry no data,
        // so an ignored extra field there is harmless.)
        let r = serde_json::from_str::<Request>(r#"{"request":"focus_panel","id":1,"x":2}"#);
        assert!(r.is_err());
        assert!(serde_json::from_str::<Request>(r#"{"request":"nope"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"request":"spawn"}"#).is_err());
    }
}
