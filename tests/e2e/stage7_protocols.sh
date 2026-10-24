#!/usr/bin/env bash
# Stage 7.5 desktop protocols, each checked with the tool people use it with.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
space_start --render 2d --log-level debug
WL=$(space_wayland_display)
globals=$(WAYLAND_DISPLAY="$WL" wayland-info 2>/dev/null | sed -n "s/.*interface: '\([^']*\)'.*/\1/p")
has() { grep -qx "$1" <<<"$globals" || fail "global $1 missing"; }

# ---- xdg-decoration: server-side, so foot draws no title bar of its own.
has zxdg_decoration_manager_v1
kc spawn foot >/dev/null
wait_windows 1
sleep 1
S=$(shot s7-ssd-foot)
# foot sits at (20, 20) in 2D. Its CSD title bar would be a light strip at the top; with
# server-side decorations the first rows are the terminal's dark background.
$PX near "$S" 700 26 20 20 20 40 >/dev/null || fail "foot still draws a title bar: $($PX "$S" 700 26)"
pass "xdg-decoration: foot uses server-side mode (no title bar)"
kc close 1 >/dev/null

space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
