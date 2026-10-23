#!/usr/bin/env bash
# Stage 1 acceptance: real clients connect, draw, get input, clipboard works.
source "$(dirname "$0")/lib.sh"

space_start --render 2d
WD=$(space_wayland_display)

kc spawn foot >/dev/null
wait_windows 1
sleep 0.5
kc input type 'echo spacewm-typing-works'
kc input key Return
sleep 0.5
shot s1-typing

# Second terminal via keybind (headless isn't nested, so the mod key is Super).
kc input key 'Super+Return'
wait_windows 2 || fail "Super+Return did not open a terminal"
pass "keybind opens a terminal"

# Focus follows clicks: window 1 is at (20,20), window 2 cascades to (60,60); x=30 is
# only inside window 1.
kc input move-to 30 300
kc input click
sleep 0.2
FOCUSED=$(kc --json list | python3 -c 'import json,sys; d=json.load(sys.stdin); print([p["id"] for p in d["panels"] if p["focused"]])')
[ "$FOCUSED" = "[1]" ] || fail "click did not focus window 1 (focused: $FOCUSED)"
pass "click focuses the window under the pointer"

# Clipboard: set it with wl-copy (data-control), paste into foot with Ctrl+Shift+V.
WAYLAND_DISPLAY=$WD wl-copy "pasted-from-clipboard" &
sleep 0.5
kc input key 'Ctrl+Shift+v'
kc input key Return
sleep 0.5
GOT=$(WAYLAND_DISPLAY=$WD timeout 2 wl-paste --no-newline || true)
[ "$GOT" = "pasted-from-clipboard" ] || fail "wl-paste returned '$GOT'"
pass "clipboard round-trip through data-control"
shot s1-clipboard

# Frame callbacks: htop must keep redrawing.
kc input type 'htop -d 5'
kc input key Return
sleep 1.5
A=$(shot s1-htop-a); sleep 1.2; B=$(shot s1-htop-b)
cmp -s "$A" "$B" && fail "htop did not redraw (frame callbacks broken?)"
pass "htop keeps animating (frame callbacks work)"
kc input key q

# Close the focused window with Super+Shift+Q.
kc input key 'Super+Shift+q'
sleep 0.5
N=$(kc --json list | grep -o '"id":' | wc -l)
[ "$N" -eq 1 ] || fail "Super+Shift+Q left $N windows"
pass "Super+Shift+Q closes the focused window"

space_stop || fail "spacewm exited with an error"
pass "clean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "no errors in log"
