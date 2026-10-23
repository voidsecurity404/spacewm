#!/usr/bin/env bash
# GTK popup menus are drawn into their window's panel and can be used in Window mode.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
space_start --render 3d
kc spawn gtk3-widget-factory >/dev/null
wait_windows 1
sleep 2
# Spawned by us, so it is focused (Window mode). Look at it at the 1:1 distance, where
# one window px is one screen px and the window center is the screen center.
face_panel widget-factory "$PIXEL_PERFECT"
sleep 0.3
read -r W H < <(kc --json list | python3 -c "import json,sys; p=json.load(sys.stdin)['panels'][0]; print(*p['size_px'])")
A=$(shot s3-gtk)
# The "Andrea" combo box sits at (464, 250) in window coordinates.
read -r SX SY < <(python3 -c "print(640 + 464 - $W/2, 360 + 250 - $H/2)")
kc input move-to "$SX" "$SY"
kc input click
sleep 0.8
B=$(shot s3-gtk-menu)
D=$($PX diff "$A" "$B")
python3 -c "exit(0 if $D > 0.002 else 1)" || fail "no popup on the panel (diff $D)"
pass "combo box popup appears on the GTK panel (diff $D)"
kc input key Escape
sleep 0.4
C=$(shot s3-gtk-closed)
D2=$($PX diff "$B" "$C")
python3 -c "exit(0 if $D2 > 0.002 else 1)" || fail "popup did not disappear (diff to menu shot $D2)"
pass "menu disappears from the panel when closed"
space_stop || fail "unclean exit"
