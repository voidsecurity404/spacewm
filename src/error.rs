//! Crate-wide error type.
//!
//! Modules return `Result<T, SpaceError>` (thiserror: a typed enum callers can match
//! on). main.rs works in `anyhow` and adds `.context(...)` so the final message
//! reads like "failed to start winit backend: <cause>".

// Variants other than Io are constructed from Stage 1 on.
#[allow(dead_code)]
#[derive(thiserror::Error, Debug)]
pub enum SpaceError {
    #[error("backend: {0}")]
    Backend(String),
    #[error("render: {0}")]
    Render(String),
    #[error("wayland: {0}")]
    Wayland(String),
    #[error("config: {0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
