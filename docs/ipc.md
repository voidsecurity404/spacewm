# spacewm IPC

spacewm listens on a Unix socket, `$XDG_RUNTIME_DIR/spacewm-$WAYLAND_DISPLAY.sock`
(or `--ipc-socket PATH`). Programs started by spacewm get its path in `SPACEWM_SOCKET`.
`spacectl` is the command-line client; anything that can write JSON to a socket can talk
to it too, for example `socat - UNIX-CONNECT:$SPACEWM_SOCKET`.

## Wire format

Newline-delimited JSON: send one request object per line, get one response object per
line back, in order. Requests are tagged by `"request"`, responses by `"response"`.
Field names are snake_case; unknown fields are an error.

```
→ {"request":"version"}
← {"response":"version","version":"0.0.1","backend":"drm"}
```

## Requests

| request | fields | response |
|---|---|---|
| `version` | | `version` |
| `list_panels` | | `panels` |
| `focus_panel` | `id` | `ok` (3D: enters Window mode on that panel) |
| `close_panel` | `id` | `ok` (asks the app to close; it may ask to save first) |
| `move_panel` | `id`, `position: [x, y, z]` (m), `yaw_deg?` | `ok` |
| `scale_panel` | `id`, `pixels_per_unit` (200..3000) | `ok` |
| `spawn` | `command`, `shell?` (default false) | `spawned` with `pid` |
| `set_mode` | `mode`: `"walk"` or `{"window": id}` | `ok` (3D only) |
| `teleport` | `position: [x, y, z]`, `yaw_deg?`, `pitch_deg?` | `ok` (camera; puts you in fly mode) |
| `reload_config` | | `ok`, or `error` with the config problems |
| `screenshot` | `path` (absolute, inside $HOME), `force?` | `screenshot` with `path`, `width`, `height` |
| `set_block` / `get_block` | `pos: [x, y, z]`, `block` | `ok` / `block` |
| `subscribe` | `events: [...]` | `ok`, then a stream of `event` responses |
| `input` | `action` (see below) | `ok` (only with `spacewm --allow-input-injection`) |
| `quit` | | `ok` |

Responses: `ok`, `error` (`message`), `version`, `panels` (`panels: [PanelInfo]`),
`spawned`, `screenshot`, `block`, `event`.

`PanelInfo`: `id`, `app_id`, `title`, `position` (metres; 2D mode: pixels), `yaw_deg`,
`pixels_per_unit`, `visible`, `size_px: [w, h]`, `focused`, `kind` (`"wayland"`/`"x11"`).

### Examples

```
→ {"request":"list_panels"}
← {"response":"panels","panels":[{"id":3,"app_id":"foot","title":"~","position":[0.0,21.7,-1.2],
   "yaw_deg":0.0,"pixels_per_unit":800.0,"visible":true,"size_px":[994,700],"focused":true,
   "kind":"wayland"}]}

→ {"request":"move_panel","id":3,"position":[0,2,-4],"yaw_deg":90}
← {"response":"ok"}

→ {"request":"spawn","command":"foot -e htop"}
← {"response":"spawned","pid":4242}

→ {"request":"set_mode","mode":{"window":3}}
← {"response":"ok"}

→ {"request":"focus_panel","id":99}
← {"response":"error","message":"no panel 99"}
```

### Events

`subscribe` turns the connection into an event stream (send it last on a connection).
Kinds: `panel_opened` (`id`, `app_id`), `panel_closed` (`id`), `focus_changed` (`id` or
null), `mode_changed` (`mode`: `"walk"`/`"window"`), `config_reloaded`.

```
→ {"request":"subscribe","events":["panel_opened","panel_closed","focus_changed"]}
← {"response":"ok"}
← {"response":"event","event":{"event":"panel_opened","id":4,"app_id":null}}
← {"response":"event","event":{"event":"focus_changed","id":4}}
```

### Input injection (tests)

`{"request":"input","action":{...}}` with `action` one of
`{"action":"key","combo":"Super+Return","state":"tap"}` (`press`/`release`/`tap`),
`{"action":"type","text":"hello"}`, `{"action":"move_to","x":640,"y":360}`,
`{"action":"move_by","dx":10,"dy":0}`, `{"action":"button","button":"left","state":"tap"}`,
`{"action":"scroll","dx":0,"dy":1}`.

## spacectl

```
spacectl list                      # table; --json for the raw response
spacectl move 3 0 2 -4 --yaw 90
spacectl scale 3 1200
spacectl mode window 3 | spacectl mode walk
spacectl spawn foot -e htop        # argv, no shell
spacectl spawn --shell "ls | wc -l > ~/n.txt"   # needs general.ipc_allow_shell
spacectl screenshot ~/shot.png [--force]
spacectl reload-config
spacectl watch                     # prints events until spacewm exits
```

`spacectl` exits with status 1 when spacewm answers with an error.

## Security

Anything that can use this socket can start programs as you, so:

- The socket is created with mode 0600 inside `$XDG_RUNTIME_DIR` (itself 0700), and every
  connection's peer is checked with `SO_PEERCRED`: other users — root included — are
  rejected even if the permissions were loosened.
- A request line may be at most 64 KiB; at most 16 connections at once; a client that
  doesn't read its responses (over 1 MiB queued) is dropped. All socket I/O is
  non-blocking, so a stuck client can't freeze the compositor.
- Malformed JSON, wrong types, unknown fields and non-finite or out-of-range numbers get
  an `error` response; nothing a client sends can crash spacewm.
- `spawn` runs the command directly (split into words like a shell would, but no shell
  runs: no pipes, redirections or `$(...)`). `shell: true` is refused unless the config
  sets `general.ipc_allow_shell = true`.
- `screenshot` writes only inside `$HOME` (the path is resolved, so `..` and symlinked
  parents can't escape), never through a symlink, and never over an existing file unless
  `force` is set.
- `input` injection is off unless spacewm was started with `--allow-input-injection`
  (otherwise any of your processes could type into your terminals).
