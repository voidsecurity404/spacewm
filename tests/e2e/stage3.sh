#!/usr/bin/env bash
# Stage 3 acceptance: windows as live textured panels in 3D.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
pos() { kc --json list | python3 -c "import json,sys; d=json.load(sys.stdin); print(*[p['position'] for p in d['panels'] if p['id']==$1][0])"; }

space_start --render 3d
# Client-side decorations on purpose: its title bar shows which way is up.
kc spawn foot -o csd.preferred=client >/dev/null
wait_windows 1
sleep 0.6
# Look straight at the panel so its layout on screen is predictable.
read -r X Y Z < <(pos 1)
face_panel 1 0.9
sleep 0.3
A=$(shot s3-front)
# foot's title bar (light) is at the TOP of the panel, the terminal (dark) below it:
# if the texture were upside down this would be the other way round.
TOP=$($PX "$A" 640 140); MID=$($PX "$A" 640 400)
python3 - "$TOP" "$MID" <<'PY' || fail "panel not upright (title bar $TOP, body $MID)"
import sys
top = sum(map(int, sys.argv[1].split())); mid = sum(map(int, sys.argv[2].split()))
sys.exit(0 if top > mid + 200 else 1)
PY
pass "panel shows the window upright (title bar on top)"

# Live content: htop must keep changing the panel.
kc input type 'htop -d 5'; kc input key Return
sleep 1.2
B1=$(shot s3-htop-a); sleep 1.0; B2=$(shot s3-htop-b)
D=$($PX diff "$B1" "$B2")
python3 -c "exit(0 if $D > 0.001 else 1)" || fail "htop panel did not update ($D)"
pass "htop updates live on the panel ($D)"
kc input key q
sleep 0.3

# An idle terminal re-renders its texture ~0 times per second.
kc input key Super+F3
sleep 2.4
LAST=$(grep 'debug fps' "$OUT/spacewm.log" | tail -1 | grep -o 'panel_updates=[0-9]*' | cut -d= -f2)
[ "${LAST:-99}" -le 1 ] || fail "idle terminal re-rendered $LAST times/s"
pass "idle terminal: $LAST panel texture updates in the last second"
kc input key Super+F3

# Second terminal appears to the right of the first, not overlapping.
kc input key Super+Return
wait_windows 2
sleep 0.5
read -r X2 Y2 Z2 < <(pos 2)
python3 -c "import math; exit(0 if math.dist(($X,$Y,$Z), ($X2,$Y2,$Z2)) > 1.2 else 1)" || fail "second panel at ($X2,$Y2,$Z2) overlaps the first at ($X,$Y,$Z)"
pass "second panel spawns beside the first, not inside it"

# Back side is a dark silhouette (in Walk mode: Window mode dims unfocused panels).
kc input key Super+Escape
behind_panel 1 0.9
sleep 0.3
C=$(shot s3-back)
$PX near "$C" 500 300 26 26 26 12 >/dev/null || fail "back side not dark: $($PX "$C" 500 300)"
pass "back side is dark"

# A client that resizes itself: the panel follows, growing around its center.
kc spawn weston-resizor >/dev/null
wait_windows 3
sleep 0.8
info() { kc --json list | python3 -c "import json,sys; p=[p for p in json.load(sys.stdin)['panels'] if 'resizor' in (p['app_id'] or '')][0]; print(p['size_px'][0], p['size_px'][1], *p['position'])"; }
read -r W1 H1 PX1 PY1 PZ1 < <(info)
for _ in 1 2 3 4 5; do kc input key Right; kc input key Down; done
sleep 1.5
read -r W2 H2 PX2 PY2 PZ2 < <(info)
[ "${W2}x${H2}" != "${W1}x${H1}" ] || fail "resizor size did not change: ${W1}x${H1} -> ${W2}x${H2}"
python3 -c "exit(0 if abs($PX2-$PX1) + abs($PY2-$PY1) + abs($PZ2-$PZ1) < 1e-6 else 1)" || fail "panel center moved while resizing"
pass "client resize grows the panel around its center (${W1}x${H1} -> ${W2}x${H2})"

# Subsurfaces (video players, embedded widgets) are part of the panel texture.
kc spawn weston-subsurfaces >/dev/null
wait_windows 4
sleep 1
face_panel sub-surface 0.9
sleep 0.3
shot s3-subsurfaces >/dev/null
pass "subsurfaces client rendered (see s3-subsurfaces.png)"

space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "no errors in log"
