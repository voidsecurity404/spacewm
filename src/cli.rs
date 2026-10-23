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
    /// winit = window inside the current desktop; drm = directly on a TTY.
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
    }

    #[test]
    fn defaults() {
        let cli = Cli::try_parse_from(["spacewm"]).expect("no arguments is valid");
        assert_eq!(cli.backend, BackendKind::Winit);
        assert!(cli.spawn.is_empty());
    }

    #[test]
    fn render_2d() {
        let cli = Cli::try_parse_from(["spacewm", "--render", "2d"]).expect("valid");
        assert_eq!(cli.render, RenderMode::Flat2D);
    }
}
