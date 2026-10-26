#!/usr/bin/env bash
# Helpers for end-to-end tests: run spacewm headless, drive it with spacectl, take screenshots.
# Usage: source tests/e2e/lib.sh; space_start [extra spacewm args]; kc list; shot name; space_stop
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${SPACE_TEST_OUT:-$HOME/spacewm-test}"
mkdir -p "$OUT"
: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}"
SOCK="$XDG_RUNTIME_DIR/spacewm-e2e-$$.sock"
SPACEWM="$ROOT/target/debug/spacewm"
SPACECTL="$ROOT/target/debug/spacectl"
SPACE_PID=""

kc() { "$SPACECTL" --socket "$SOCK" "$@"; }

space_start() {
    # Never touch the user's real saved layout: tests get their own file unless they
    # pass --layout themselves (later arguments win in clap).
    local layout=(--layout "$OUT/layout-$$.json")
    case " $* " in *" --layout "*) layout=() ;; esac
    # Same for the config: tests must never create or read ~/.config/spacewm/config.toml.
    local config=(--config "$OUT/config-$$.toml")
    case " $* " in *" --config "*) config=() ;; esac
    # Tests were written for the block world (its terrain, spawn and building): that is
    # their default; the map's own test passes --map dust.
    local map=(--map voxel)
    case " $* " in *" --map "*) map=() ;; esac
    "$SPACEWM" --backend headless --allow-input-injection --ipc-socket "$SOCK" \
        "${layout[@]}" "${config[@]}" "${map[@]}" "$@" >"$OUT/spacewm.log" 2>&1 &
    SPACE_PID=$!
    for _ in $(seq 50); do
        [ -S "$SOCK" ] && kc version >/dev/null 2>&1 && return 0
        sleep 0.1
    done
    echo "spacewm did not start; log:" >&2
    tail -20 "$OUT/spacewm.log" >&2
    return 1
}

space_stop() {
    if [ -n "$SPACE_PID" ]; then
        kc quit >/dev/null 2>&1 || kill "$SPACE_PID" 2>/dev/null || true
        wait "$SPACE_PID" 2>/dev/null
    fi
    local code=$?
    SPACE_PID=""
    # After spacewm is gone: it saves the layout as it exits, and a leftover file would
    # reopen this run's apps in the next start.
    rm -f "$OUT/layout-$$.json" "$OUT/config-$$.toml"
    return $code
}

# The WAYLAND_DISPLAY spacewm listens on (from its log).
space_wayland_display() {
    sed -n 's/.*spacewm listening on \([^ ]*\).*/\1/p' "$OUT/spacewm.log" | head -1
}

shot() {
    rm -f "$OUT/$1.png"
    kc screenshot "$OUT/$1.png" >/dev/null
    echo "$OUT/$1.png"
}

# Wait until `kc list` shows at least N windows.
wait_windows() {
    local n=$1
    for _ in $(seq 100); do
        [ "$(kc --json list | grep -o '"id":' | wc -l)" -ge "$n" ] && return 0
        sleep 0.1
    done
    echo "timed out waiting for $n windows" >&2
    kc list >&2
    return 1
}

fail() { echo "FAIL: $*" >&2; space_stop || true; exit 1; }
pass() { echo "ok - $*"; }

trap 'space_stop >/dev/null 2>&1 || true' EXIT

# Put the camera DIST metres in front of panel ID (matched by id or app_id substring),
# looking straight at it.
face_panel() {
    local sel=$1 dist=${2:-0.9}
    local args
    args=$(kc --json list | python3 -c "
import json, math, sys
sel, dist = sys.argv[1], float(sys.argv[2])
ps = json.load(sys.stdin)['panels']
p = [p for p in ps if str(p['id']) == sel or sel in (p['app_id'] or '')][0]
y = math.radians(p['yaw_deg'])
x, py, z = p['position']
# Front normal is (sin yaw, 0, cos yaw); the camera looks back along -normal: camera yaw = -panel yaw.
print(x + math.sin(y) * dist, py, z + math.cos(y) * dist, -p['yaw_deg'])
" "$sel" "$dist")
    read -r cx cy cz cyaw <<<"$args"
    kc teleport "$cx" "$cy" "$cz" --yaw "$cyaw" --pitch 0
}

# Ids of panels with keyboard focus, e.g. "[1]" (empty list in Walk mode).
focused() { kc --json list | python3 -c "import json,sys; print([p['id'] for p in json.load(sys.stdin)['panels'] if p['focused']])"; }

# The distance at which a panel shows 1 window px per screen px on the 1280x720 headless
# output: 720 / (2 * 800 * tan(37.5 deg)).
PIXEL_PERFECT=0.58644

# Put the camera DIST metres BEHIND panel SEL, looking at its back.
behind_panel() {
    local sel=$1 dist=${2:-0.9}
    local args
    args=$(kc --json list | python3 -c "
import json, math, sys
sel, dist = sys.argv[1], float(sys.argv[2])
p = [p for p in json.load(sys.stdin)['panels'] if str(p['id']) == sel or sel in (p['app_id'] or '')][0]
y = math.radians(p['yaw_deg'])
x, py, z = p['position']
# Behind = along -normal; looking along +normal means camera yaw = 180 - panel yaw.
print(x - math.sin(y) * dist, py, z - math.cos(y) * dist, 180 - p['yaw_deg'])
" "$sel" "$dist")
    read -r cx cy cz cyaw <<<"$args"
    kc teleport "$cx" "$cy" "$cz" --yaw "$cyaw" --pitch 0
}
