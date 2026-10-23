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
    --allow-input-injection --ipc-socket "$SOCK" >"$OUT/winit.log" 2>&1 &
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
echo "ok - winit nested backend renders, screenshots, exits cleanly"
