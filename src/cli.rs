//! Command-line interface definitions (clap derive).

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

/// Which backend spacewm talks to: where pixels go and where input comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendKind {
    /// Runs as a normal window inside the current desktop (safe for development).
    Winit,
    /// Runs directly on a TTY with DRM/KMS, libinput and libseat (a real session).
    Drm,
    /// No screen: renders offscreen on the GPU. For automated tests (screenshots + IPC).
    Headless,
}

/// How the scene is drawn. `2d` keeps the plain Stage 1 path for debugging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RenderMode {
    #[value(name = "2d")]
    Flat2D,
    #[value(name = "3d")]
    Scene3D,
}

#[derive(Debug, Clone, Parser)]
#[command(name = "spacewm", version, about = "A 3D Wayland compositor")]
pub struct Cli {
    /// winit = window inside the current desktop; drm = directly on a TTY;
    /// headless = offscreen, for tests.
    #[arg(long, value_enum, default_value_t = BackendKind::Winit)]
    pub backend: BackendKind,

    /// Log level for spacewm itself. If RUST_LOG is set, it wins.
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Command to start once the compositor socket is ready (repeatable).
    #[arg(long = "spawn")]
    pub spawn: Vec<String>,

    /// Render mode: 2d (flat debug view) or 3d (the world).
    #[arg(long, value_enum, default_value_t = RenderMode::Scene3D)]
    pub render: RenderMode,

    /// Path to a config file (used from Stage 7).
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Load GLSL shaders from this directory instead of the built-in copies, and reload
    /// them with Mod+F5. For editing shaders without recompiling.
    #[arg(long)]
    pub shader_dir: Option<PathBuf>,

    /// Where panel placements and block edits are saved
    /// (default: $XDG_STATE_HOME/spacewm/layout.json).
    #[arg(long)]
    pub layout: Option<PathBuf>,

    /// World generation seed (the same seed always gives the same island).
    #[arg(long, default_value_t = crate::world::terrain::DEFAULT_SEED)]
    pub world_seed: u64,

    /// DRM backend: GPU to use (default: the boot GPU of the seat), e.g. /dev/dri/card1.
    #[arg(long)]
    pub gpu: Option<PathBuf>,

    /// Start on the DRM backend, show the world for 10 seconds without clients, exit.
    /// Run this first from a TTY to check that your GPU and monitor work.
    #[arg(long)]
    pub drm_test: bool,

    /// List GPUs, connectors and modes, then exit. Safe to run anywhere: it does not
    /// take over the display.
    #[arg(long)]
    pub drm_probe: bool,

    /// DRM backend without input devices (testing on a virtual display).
    #[arg(long, hide = true)]
    pub drm_no_input: bool,

    /// Headless backend: output size as WIDTHxHEIGHT.
    #[arg(long, default_value = "1280x720", value_parser = parse_size)]
    pub headless_size: (i32, i32),

    /// Headless backend: GPU render node (default: the first /dev/dri/renderD*).
    #[arg(long)]
    pub headless_device: Option<PathBuf>,

    /// IPC socket path (default: $XDG_RUNTIME_DIR/spacewm-$WAYLAND_DISPLAY.sock).
    #[arg(long)]
    pub ipc_socket: Option<PathBuf>,

    /// Allow `spacectl input ...` to inject keyboard/mouse events. Off by default: any
    /// process of your user could otherwise type into your terminals. For tests.
    #[arg(long)]
    pub allow_input_injection: bool,
}

/// Parse "1280x720" into (1280, 720).
pub fn parse_size(s: &str) -> Result<(i32, i32), String> {
    let (w, h) = s.split_once('x').ok_or_else(|| format!("expected WIDTHxHEIGHT, got '{s}'"))?;
    let w: i32 = w.trim().parse().map_err(|_| format!("bad width in '{s}'"))?;
    let h: i32 = h.trim().parse().map_err(|_| format!("bad height in '{s}'"))?;
    if !(64..=16384).contains(&w) || !(64..=16384).contains(&h) {
        return Err(format!("size out of range (64..=16384): '{s}'"));
    }
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_field() {
        let cli = Cli::try_parse_from([
            "spacewm",
            "--backend",
            "drm",
            "--spawn",
            "foot",
            "--spawn",
            "htop",
        ])
        .expect("valid arguments");
        assert_eq!(cli.backend, BackendKind::Drm);
        assert_eq!(cli.log_level, "info");
        assert_eq!(cli.spawn, vec!["foot".to_string(), "htop".to_string()]);
        assert_eq!(cli.render, RenderMode::Scene3D);
        assert!(cli.config.is_none());
        assert!(!cli.allow_input_injection);
    }

    #[test]
    fn defaults() {
        let cli = Cli::try_parse_from(["spacewm"]).expect("no arguments is valid");
        assert_eq!(cli.backend, BackendKind::Winit);
        assert!(cli.spawn.is_empty());
        assert_eq!(cli.headless_size, (1280, 720));
    }

    #[test]
    fn render_2d() {
        let cli = Cli::try_parse_from(["spacewm", "--render", "2d"]).expect("valid");
        assert_eq!(cli.render, RenderMode::Flat2D);
    }

    #[test]
    fn size_parsing() {
        assert_eq!(parse_size("1920x1080"), Ok((1920, 1080)));
        assert!(parse_size("1920").is_err());
        assert!(parse_size("0x100").is_err());
        assert!(parse_size("axb").is_err());
    }
}
