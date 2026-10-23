# spacewm

spacewm is a Wayland compositor written from scratch in Rust on top of smithay. It runs straight from a TTY, with no other desktop or window manager underneath, and shows application windows (terminals, browsers) as panels inside a Minecraft-like voxel world: you walk around, look at a window, click it and type.

## Build and run

```sh
cargo build
cargo run -- --backend winit          # nested window inside your current desktop
cargo run -- --backend winit --spawn foot
just check                            # fmt + clippy -D warnings + tests
```

Logs: stderr and `~/.local/state/spacewm/spacewm.log` (previous run in `spacewm.log.old`).

## Stages

0. Project skeleton: CLI, logging, errors, event loop
1. Minimal 2D compositor inside winit
2. 3D scene and camera
3. Windows as textured 3D panels
4. Input modes and raycast focus
5. Voxel world and panel placement
6. Real session on a TTY (DRM/KMS)
7. XWayland, config, IPC (`spacectl`) and polish
