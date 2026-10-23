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
    "$SPACEWM" --backend headless --allow-input-injection --ipc-socket "$SOCK" "$@" \
        >"$OUT/spacewm.log" 2>&1 &
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
    [ -n "$SPACE_PID" ] || return 0
    kc quit >/dev/null 2>&1 || kill "$SPACE_PID" 2>/dev/null || true
    wait "$SPACE_PID" 2>/dev/null
    local code=$?
    SPACE_PID=""
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
