#!/usr/bin/env bash
# Doing less work without showing less:
# - a near window without popups is drawn from the app's own texture (no copy), and it
#   looks exactly like the copy;
# - its mipmaps are skipped up close and rebuilt correctly when you walk away;
# - apps on far panels draw at render.far_panel_fps.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
CFG="$OUT/config-eff-$$.toml"
trap 'rm -f "$CFG"; space_stop >/dev/null 2>&1 || true' EXIT
st() { kc --json stats | python3 -c "import json,sys; print(json.load(sys.stdin)['stats']['$1'])"; }
setting() { sed -i "s/^$1 = .*/$1 = $2/" "$CFG"; sleep 1.5; }

rm -f "$CFG"
space_start --render 3d --config "$CFG"
# A still, lopsided picture (a diagonal of #), so a flipped or shifted texture shows.
kc spawn foot sh -c 'echo "spacewm direct test"; for i in $(seq 1 14); do printf "%*s#\n" $((i * 3)) ""; done; exec sleep 600' >/dev/null
wait_windows 1
sleep 1.5
face_panel 1 "$PIXEL_PERFECT"
sleep 1
[ "$(st direct_panels)" = 1 ] || fail "near terminal not drawn directly (direct_panels=$(st direct_panels))"
A=$(shot s16-direct)
setting direct_panels false
[ "$(st direct_panels)" = 0 ] || fail "render.direct_panels = false did not switch to the copy"
B=$(shot s16-copy)
D=$($PX diff "$A" "$B")
python3 -c "exit(0 if $D < 0.002 else 1)" || fail "direct and copied panel look different (diff $D)"
setting direct_panels true
[ "$(st direct_panels)" = 1 ] || fail "direct drawing did not come back"
pass "near window drawn without a copy, identical to the copy (diff $D)"

# Typing while near changes the picture but skips the mipmaps (on the copy: a window
# with a menu open, say); from 4 m away the rebuilt mipmaps must show the same as
# freshly made textures.
setting direct_panels false
kc input type "abc mipmap test"
sleep 0.5
kc mode walk
face_panel 1 4
sleep 1
[ "$(st direct_panels)" = 0 ] || fail "far panel still drawn directly"
C=$(shot s16-far)
setting panel_mipmaps false
setting panel_mipmaps true
E=$(shot s16-far-fresh)
D2=$($PX diff "$C" "$E")
python3 -c "exit(0 if $D2 < 0.002 else 1)" || fail "stale mipmaps after walking away (diff $D2)"
setting direct_panels true
pass "mipmaps skipped up close are rebuilt on walking away (diff $D2)"

# Far panels: their app draws at far_panel_fps (15) instead of every frame.
kc spawn foot sh -c 'while :; do seq 1 40; sleep 0.01; done' >/dev/null
wait_windows 2
kc mode walk
face_panel 2 14
sleep 3
far=$(st panel_updates_per_s)
python3 -c "exit(0 if 8 <= $far <= 18 else 1)" || fail "far busy panel updated $far/s, wanted ~15"
setting far_panel_fps 0
sleep 2
free=$(st panel_updates_per_s)
python3 -c "exit(0 if $free >= 30 else 1)" || fail "far_panel_fps = 0 still limited ($free/s)"
pass "far busy terminal drawn at $far/s (unlimited: $free/s)"

space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
