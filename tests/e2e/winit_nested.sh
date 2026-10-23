#!/usr/bin/env bash
# The winit backend, nested inside a headless weston (so no window pops up on your desktop).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${SPACE_TEST_OUT:-$HOME/spacewm-test}"; mkdir -p "$OUT"
HOST="wl-spacewm-host-$$"
SOCK="$XDG_RUNTIME_DIR/spacewm-winit-$$.sock"
K() { "$ROOT/target/debug/spacectl" --socket "$SOCK" "$@"; }

weston --backend=headless --renderer=gl --width=1400 --height=900 --socket="$HOST" --no-config >"$OUT/weston.log" 2>&1 &
WP=$!
trap 'kill $WP 2>/dev/null || true' EXIT
for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/$HOST" ] && break; sleep 0.1; done

WAYLAND_DISPLAY="$HOST" "$ROOT/target/debug/spacewm" --backend winit --render 2d \
    --allow-input-injection --ipc-socket "$SOCK" --layout "$OUT/layout-winit.json" >"$OUT/winit.log" 2>&1 &
KP=$!
for _ in $(seq 50); do K version >/dev/null 2>&1 && break; sleep 0.1; done

K spawn foot >/dev/null
sleep 2
K input type 'echo nested-winit-ok'; K input key Return; sleep 0.7
rm -f "$OUT/winit.png"; K screenshot "$OUT/winit.png"
K input type 'echo after-screenshot'; K input key Return; sleep 0.5
rm -f "$OUT/winit2.png"; K screenshot "$OUT/winit2.png"
K quit; wait $KP
if grep -E "ERROR|WARN.*submit|panicked" "$OUT/winit.log"; then echo "FAIL: errors in winit log"; exit 1; fi
echo "ok - winit nested backend (2D) renders, screenshots, exits cleanly"

# 3D through the winit surface (Flipped180 output transform): sky must be on top.
WAYLAND_DISPLAY="$HOST" "$ROOT/target/debug/spacewm" --backend winit --render 3d \
    --allow-input-injection --ipc-socket "$SOCK" --layout "$OUT/layout-winit.json" >"$OUT/winit3d.log" 2>&1 &
KP=$!
for _ in $(seq 50); do K version >/dev/null 2>&1 && break; sleep 0.1; done
K teleport 0 30 40 --yaw 0 --pitch -10
sleep 0.5
rm -f "$OUT/winit3d.png"; K screenshot "$OUT/winit3d.png" >/dev/null
K quit; wait $KP
PX="python3 $(dirname "$0")/pixel.py"
$PX near "$OUT/winit3d.png" 640 20 135 191 242 30 >/dev/null || { echo "FAIL: winit 3D: no sky on top"; exit 1; }
if grep -E "ERROR|panicked" "$OUT/winit3d.log"; then echo "FAIL: errors in winit 3D log"; exit 1; fi
echo "ok - winit nested backend (3D) renders upright"
