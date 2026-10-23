# spacewm

3D Wayland compositor written from scratch in Rust. Not based on any existing WM/DE.

## Stack
- smithay (Wayland server, input, backends)
- smithay GlesRenderer + custom GLSL shaders + glam (3D rendering)
- winit backend for development, DRM/KMS + libinput + libseat for real sessions
- tracing for logs

## Rules
- Always keep the project compiling. Run `cargo build` and `cargo clippy` after changes.
- Small modules: backend/, render/, world/, input/, wayland/ (protocol handlers), config/.
- No `unwrap()` in non-test code; use `anyhow` / `thiserror`.
- Explain new Wayland/Smithay concepts briefly in comments, I am learning.
- Before big changes, show a plan and wait for approval.
- Never run commands that need sudo or touch system files without asking.

## Run
- Dev: `cargo run -- --backend winit` (nested window)
- Test clients: `WAYLAND_DISPLAY=spacewm-0 foot` or `weston-terminal`
