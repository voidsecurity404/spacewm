#!/usr/bin/env bash
# Panel commands from Walk mode: rotate, reset, scale, hide/show.
source "$(dirname "$0")/lib.sh"
info() { kc --json list | python3 -c "import json,sys; p=json.load(sys.stdin)['panels'][0]; print(round(p['yaw_deg'],1), round(p['pixels_per_unit'],1), p['visible'])"; }
rm -f "$OUT/layout-ops.json"
space_start --render 3d --layout "$OUT/layout-ops.json"
kc spawn foot >/dev/null
wait_windows 1
sleep 0.6
kc input key Super+Escape   # Walk mode, crosshair on the new panel
sleep 0.2
read -r Y0 S0 V0 < <(info)
kc input key Super+r; sleep 0.2
read -r Y1 _ _ < <(info)
python3 -c "import sys; d=($Y1-$Y0+540)%360-180; sys.exit(0 if abs(abs(d)-90) < 0.5 else 1)" || fail "Mod+R turned $Y0 -> $Y1"
pass "Mod+R rotates the panel 90 degrees ($Y0 -> $Y1)"
# The panel now faces sideways, so the crosshair hits its edge-on side: target it via focus.
kc focus 1; kc input key Super+Shift+r; sleep 0.2
read -r Y2 _ _ < <(info)
python3 -c "import sys; sys.exit(0 if abs($Y2-$Y0) < 0.5 else 1)" || fail "Mod+Shift+R did not reset ($Y2 vs $Y0)"
pass "Mod+Shift+R faces the camera again"
kc input key Super+equal; sleep 0.2
read -r _ S1 _ < <(info)
python3 -c "import sys; sys.exit(0 if abs($S1 - $S0/1.1) < 0.5 else 1)" || fail "Mod+= scale $S0 -> $S1"
kc input key Super+minus; sleep 0.2
read -r _ S2 _ < <(info)
python3 -c "import sys; sys.exit(0 if abs($S2 - $S0) < 0.5 else 1)" || fail "Mod+- scale back $S1 -> $S2"
pass "Mod+=/Mod+- scale the panel by 10% ($S0 -> $S1 -> $S2 px/m)"
kc input key Super+h; sleep 0.2
read -r _ _ V1 < <(info)
[ "$V1" = "False" ] || fail "Mod+H did not hide"
[ "$(focused)" = "[]" ] || fail "hidden panel kept focus"
kc input key Super+h; sleep 0.2
read -r _ _ V2 < <(info)
[ "$V2" = "True" ] || fail "Mod+H with nothing targeted did not show hidden panels"
pass "Mod+H hides the panel and shows it again"
space_stop || fail "unclean exit"
