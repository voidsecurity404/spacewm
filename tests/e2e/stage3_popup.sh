#!/usr/bin/env bash
# GTK popup menus are drawn into their window's panel (Stage 3 still uses 2D input).
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
space_start --render 3d
kc spawn gtk3-widget-factory >/dev/null
wait_windows 1
sleep 2
read -r X Y Z < <(kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['position'])")
kc teleport "$X" "$Y" "$(python3 -c "print($Z + 1.6)")" --yaw 0 --pitch 0
sleep 0.3
A=$(shot s3-gtk)
# Left-click the "Andrea" combo box (2D coordinates: Stage 3 still uses 2D input; the
# right button is reserved for looking around in 3D) — it opens an xdg_popup menu.
kc input move-to 484 270
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
