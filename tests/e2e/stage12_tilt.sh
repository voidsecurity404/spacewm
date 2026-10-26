#!/usr/bin/env bash
# Tilting windows: Mod+PageDown / Mod+PageUp lean a window to look down / up, Mod+Shift+R
# stands it upright again, clicks still land on a tilted window, and a carried window
# leans by itself to face you when you hold it up high.
source "$(dirname "$0")/lib.sh"
tilt() { kc --json list | python3 -c "import json,sys; print(round(json.load(sys.stdin)['panels'][0]['tilt_deg']))"; }
near() { python3 -c "import sys; sys.exit(0 if abs($1 - $2) <= $3 else 1)"; }

space_start --render 3d --log-level debug
kc spawn foot >/dev/null
wait_windows 1
sleep 0.8
kc mode walk
face_panel foot 1.5
sleep 0.3
[ "$(tilt)" = 0 ] || fail "a new window is not upright: $(tilt)"
kc input key Super+Next
sleep 0.2
[ "$(tilt)" = 15 ] || fail "Mod+PageDown did not tilt 15 degrees down: $(tilt)"
kc input key Super+Next
sleep 0.2
[ "$(tilt)" = 30 ] || fail "second Mod+PageDown: $(tilt)"
grep -q 'panel tilted panel=1 tilt_deg=30' "$OUT/spacewm.log" || fail "no 'panel tilted' log"
shot s12-tilted-down >/dev/null
pass "Mod+PageDown tilts the window down in 15 degree steps"

# Clicks follow the tilted surface: aiming at the middle enters the window.
face_panel foot 1.5
sleep 0.3
kc input click
sleep 0.3
[ "$(focused)" = "[1]" ] || fail "a click on the tilted window did not enter it: $(focused)"
kc mode walk
sleep 0.3
pass "a tilted window still takes clicks"

face_panel foot 1.5
sleep 0.2
for _ in 1 2 3; do kc input key Super+Prior; sleep 0.15; done
[ "$(tilt)" = -15 ] || fail "three Mod+PageUp from 30 should give -15: $(tilt)"
for _ in $(seq 8); do kc input key Super+Prior; sleep 0.1; done
[ "$(tilt)" = -75 ] || fail "the tilt is not capped at 75 degrees: $(tilt)"
kc input key Super+Shift+r
sleep 0.2
[ "$(tilt)" = 0 ] || fail "Mod+Shift+R did not make it upright: $(tilt)"
pass "Mod+PageUp tilts up (at most 75 degrees), Mod+Shift+R makes it upright"

# Carry it, look up: it leans down to face the eyes (by the angle minus the 8 degree
# dead zone); dropped in the air it stays that way.
kc teleport 0.5 40 0.5 --yaw 0 --pitch 0
sleep 0.2
kc move 1 0.5 40 -1.5 --yaw 0 >/dev/null
sleep 0.2
kc input key Super+g
sleep 0.2
kc teleport 0.5 40 0.5 --yaw 0 --pitch 40
sleep 0.5
kc input key Super+g
sleep 0.3
T=$(tilt)
near "$T" 32 3 || fail "carried up high it should lean about 32 degrees down: $T"
shot s12-auto-tilt >/dev/null
kc input key Super+g
sleep 0.2
kc teleport 0.5 40 0.5 --yaw 0 --pitch 4
sleep 0.5
kc input key Super+g
sleep 0.3
[ "$(tilt)" = 0 ] || fail "at about eye height a carried window should stay upright: $(tilt)"
pass "a carried window leans to face you, upright near eye height"
space_stop || fail "unclean exit"

# auto_tilt = false: carried windows stay upright.
CFG="$OUT/tilt-off-$$.toml"
printf '[panels]\nauto_tilt = false\n' >"$CFG"
space_start --render 3d --config "$CFG"
kc spawn foot >/dev/null
wait_windows 1
sleep 0.8
kc mode walk
kc teleport 0.5 40 0.5 --yaw 0 --pitch 0
kc move 1 0.5 40 -1.5 --yaw 0 >/dev/null
sleep 0.2
kc input key Super+g
sleep 0.2
kc teleport 0.5 40 0.5 --yaw 0 --pitch 40
sleep 0.5
kc input key Super+g
sleep 0.3
[ "$(tilt)" = 0 ] || fail "auto_tilt = false still tilted: $(tilt)"
rm -f "$CFG"
pass "auto_tilt = false keeps carried windows upright"
space_stop || fail "unclean exit"
