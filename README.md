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

## Running as a real session

Keep a second TTY or an SSH session open the first few times.

```sh
spacewm --drm-probe                     # list GPUs, connectors, modes (changes nothing)
# from a text TTY (Ctrl+Alt+F3), logged in as yourself — never with sudo:
spacewm --drm-test                      # the island for 10 s, then back to the TTY
spacewm --backend drm --spawn foot      # the full desktop
just install                          # prints the sudo commands for a login-manager entry
```

On the DRM backend: **Ctrl+Alt+Backspace** always exits, Ctrl+Alt+F1..F12 switch VTs,
the modifier is Super (Alt only when nested). If the screen ever stays black, switch to
another TTY and read the log.

## Keys

Mod is Super on a real session and Alt when nested (`general.mod_key`).

| key | does |
|---|---|
| WASD, mouse, Space | walk, look, jump (double-tap Space: fly) |
| left click | enter the panel under the crosshair; on a block: break it |
| right click | place the selected block |
| 1-5, scroll wheel | pick a hotbar slot |
| E | inventory (click a block to pick it, E or Escape to close) |
| Mod+Escape | back to walking (always works; also leaves fullscreen) |
| Mod+F | the panel fills the screen, and back |
| Mod+Tab, Mod+\` | turn to the next window (entered when it is near) |
| Mod+G / Mod+R / Mod+= / Mod+- | carry / rotate / grow / shrink a panel |
| Mod+Ctrl+= / Mod+Ctrl+- | resize the window itself |
| Mod+Return, Mod+Space | terminal, launcher |
| Mod+Shift+Q, Mod+Shift+E | close window, quit |
| Mod+K | every key on screen (Escape closes) |

Apps already open outside spacewm (on the desktop it is nested in) still open inside it:
browsers and Telegram then get their own profile in `~/.local/share/spacewm/apps/`
(log in once), D-Bus single-instance apps a private session bus. See `src/isolate.rs`.

## Config

`~/.config/spacewm/config.toml` is created with every option and its default on first
start. Saved changes apply live (keybinds, mouse, keyboard layout, fov, fog, render
distance...); a mistake keeps the previous settings and shows the error on screen.
`spacewm --check-config` validates the file.

## Control and debugging

`spacectl` talks to the running spacewm (see docs/ipc.md): `spacectl list`, `spacectl move 3 0 2 -4`,
`spacectl watch`, `spacectl stats` (fps, frame time percentiles, GPU memory). Profiling:
`cargo run --features profile` sends spans to tracy; `SPACEWM_SPAN_TIMES=1 RUST_LOG=spacewm=trace`
logs the time of each frame phase. `tests/perf/bench.sh` measures idle and busy CPU.

## Stages

0. Project skeleton: CLI, logging, errors, event loop
1. Minimal 2D compositor inside winit
2. 3D scene and camera
3. Windows as textured 3D panels
4. Input modes and raycast focus
5. Voxel world and panel placement
6. Real session on a TTY (DRM/KMS)
7. XWayland, config, IPC (`spacectl`) and polish
