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
| left click | enter the panel under the crosshair |
| right click / left click on a block | place / break blocks (only with `general.building = true`) |
| 1-5, scroll wheel, E | hotbar and inventory (only with building on) |
| Mod+Escape | back to walking (always works; also leaves fullscreen) |
| Mod+F | the panel fills the screen, and back |
| Mod+Tab, Mod+\` (hold Mod) | window list, most recent first; let go of Mod to go there |
| Mod+G / Mod+R / Mod+= / Mod+- | carry / rotate / grow / shrink a panel |
| Mod+PageDown / Mod+PageUp | tilt a panel to look down / up (Mod+Shift+R: upright) |
| F5 / F6 (walking) | view: eyes, behind, in front / emote menu (9 animations) |
| Esc (walking) | settings: world, time of day, shadows ... |
| Mod+Ctrl+= / Mod+Ctrl+- | resize the window itself |
| Mod+Return, Mod+Space | terminal, launcher |
| Mod+Shift+Q, Mod+Shift+E | close window, quit |
| Mod+K | every key on screen (Escape closes) |
| right click on a window | window menu (each entry shows its key) |
| drag a window's top edge / corner | move it / resize it |
| hold Mod | a cursor for the dock (open apps, favorites) |
| Mod+S | seated: all windows on an arc around you, no walking |
| Mod+, | settings (saved to the config file, comments kept) |
| Mod+L | lock the screen (also after 10 min idle on a real session) |
| Print / Shift+Print | screenshot of the screen / a region, saved and copied |
| media keys | volume, mic, brightness, play/pause, with an on-screen level |

Daily use: the HUD shows time, date, battery and keyboard layout; apps open when spacewm
exits are reopened at the next start (`[desktop] restore_apps`). All commands behind
these features are in the `[desktop]` config section.

An app you open that is already running outside spacewm (on the desktop it is nested in)
hands the request to that copy, whatever app it is (Steam, browsers, Telegram, GNOME
apps ...). spacewm notices it from what the start did and asks whether to close it there
and open it here (`[desktop] running_elsewhere = "ask" | "move" | "leave"`). See
`src/isolate.rs` and `src/desktop/elsewhere.rs`.

The world is a desert town map by default (CC0 Poly Haven textures, about 1500
triangles drawn in one call, sun shadows drawn once): Escape while walking opens the
settings, where the world (town or blocks), time of day and shadows change live.
See `src/world/map.rs` and `resources/maps/dust/`.

## Config

`~/.config/spacewm/config.toml` is created with every option and its default on first
start. Saved changes apply live (keybinds, mouse, keyboard layout, fov, fog, render
distance...); a mistake keeps the previous settings and shows the error on screen.
`spacewm --check-config` validates the file.

Two `[render]` options save work on apps you are not looking at closely:
`far_panel_fps` caps how often apps on distant panels redraw (default 15), and
`direct_panels` draws the window you are close to straight from the app's own texture
(default on). Both are explained in the config file itself.

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
