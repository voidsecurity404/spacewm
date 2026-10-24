#!/usr/bin/env bash
# Building and fullscreen: hotbar + inventory (E), plain clicks break/place blocks,
# Mod+F fullscreen and back, an app's own fullscreen request, Mod+grave next window.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
# Blocks are off by default.
CFG8="$OUT/config-stage8-$$.toml"
printf '[general]\nbuilding = true\n' >"$CFG8"
trap 'rm -f "$CFG8"; space_stop >/dev/null 2>&1 || true' EXIT
space_start --render 3d --config "$CFG8" --log-level debug
sleep 1.5

TOP=""
for y in $(seq 40 -1 0); do
    [ "$(kc get-block 0 "$y" 0)" != "air" ] && { TOP=$y; break; }
done
[ -n "$TOP" ] || fail "no ground under spawn"
EYE=$(python3 -c "print($TOP + 1 + 1.62)")
kc teleport 0.5 "$EYE" 2.5 --yaw 0 --pitch -60
sleep 0.4

# ---- Hotbar along the bottom while building (a number key brings it up instead of the
# dock): dark slots, the selected one framed white.
kc input key 2
sleep 0.2
H=$(shot s8-hotbar)
$PX near "$H" 640 690 0 0 0 140 >/dev/null || fail "no hotbar at the bottom: $($PX "$H" 640 690)"
pass "hotbar shown at the bottom"

# ---- Inventory: E opens it (a light gray box), a click on slot 4 picks Bricks.
kc input key e
sleep 0.4
I=$(shot s8-inventory)
grep -q "inventory opened" "$OUT/spacewm.log" || fail "E did not open the inventory"
$PX near "$I" 640 290 199 199 199 25 >/dev/null || fail "inventory box not drawn: $($PX "$I" 640 290)"
# While it is open, mouse motion moves a cursor, not the view.
kc input move-by 200 0
sleep 0.2
A=$(shot s8-inventory-moved)
python3 -c "exit(0 if $($PX diff "$I" "$A") < 0.02 else 1)" || fail "the view turned while the inventory is open"
pass "E opens the inventory; the view stays still"
# Slot 4 (index 3): box is centered; slots start PAD=18, step 74, top at PAD+TITLE_H=48.
read -r X Y < <(python3 -c "
w = 2*18 + 5*64 + 4*10; h = 2*18 + 30 + 64 + 26 + 22
ox, oy = (1280 - w)//2, (720 - h)//2
print(ox + 18 + 3*74 + 32, oy + 48 + 32)")
kc input move-to "$X" "$Y"
kc input click
sleep 0.3
grep -q 'block selected block=Bricks' "$OUT/spacewm.log" || fail "clicking slot 4 did not pick Bricks"
grep -q "inventory closed" "$OUT/spacewm.log" || fail "inventory stayed open after picking"
pass "clicking a block in the inventory selects it and closes the box"

# ---- Plain right click places it, plain left click breaks it (no Mod key).
kc teleport 0.5 "$EYE" 2.5 --yaw 0 --pitch -60
sleep 0.3
before=$(kc get-block 0 $((TOP + 1)) 1)
kc input click right
sleep 0.3
[ "$(kc get-block 0 $((TOP + 1)) 1)" = "bricks" ] || fail "right click did not place bricks (was $before, now $(kc get-block 0 $((TOP + 1)) 1))"
shot s8-bricks >/dev/null
pass "right click places the selected block"
kc input click
sleep 0.3
[ "$(kc get-block 0 $((TOP + 1)) 1)" = "air" ] || fail "left click did not break the bricks"
pass "left click breaks the block under the crosshair"

# ---- Number keys and the scroll wheel walk the hotbar.
kc input key 2
sleep 0.2
grep -q 'block selected block=Stone' "$OUT/spacewm.log" || fail "key 2 did not pick Stone"
kc input scroll 1
sleep 0.2
tail -n 30 "$OUT/spacewm.log" | grep -q 'block selected block=Planks' || fail "scroll did not move to Planks"
kc input scroll -2
sleep 0.2
tail -n 30 "$OUT/spacewm.log" | grep -q 'block selected block=Grass' || fail "scroll back did not reach Grass"
pass "keys 1-5 and the scroll wheel pick hotbar slots"

# ---- Fullscreen: Mod+F on a terminal fills the screen; Mod+Escape brings it back.
kc teleport 0.5 "$EYE" 0.5 --yaw 0 --pitch 0
kc spawn foot >/dev/null
wait_windows 1
sleep 1
kc input key Super+f
for _ in $(seq 30); do
    [ "$(kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['size_px'])")" = "1280 720" ] && break
    sleep 0.1
done
size=$(kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['size_px'])")
[ "$size" = "1280 720" ] || fail "fullscreen window is $size, not the screen size"
grep -q "panel fullscreen" "$OUT/spacewm.log" || fail "no fullscreen log line"
sleep 0.4
F=$(shot s8-fullscreen)
# No sky anywhere: the corners are the terminal's dark background.
for xy in "5 5" "1275 5" "5 715" "1275 715"; do
    # shellcheck disable=SC2086
    $PX near "$F" $xy 20 20 20 40 >/dev/null || fail "corner ($xy) is not the terminal: $($PX "$F" $xy)"
done
# Typing still goes to the terminal.
kc input type 'printf "\033[42m%200s\033[0m\n" ""'
kc input key Return
sleep 0.5
G=$(shot s8-fullscreen-typed)
python3 -c "exit(0 if $($PX diff "$F" "$G") > 0.005 else 1)" || fail "typing did not reach the fullscreen terminal"
pass "Mod+F: the terminal fills the screen ($size) and still gets keys"
kc input key Super+Escape
for _ in $(seq 30); do
    [ "$(kc --json list | python3 -c "import json,sys; print(json.load(sys.stdin)['panels'][0]['size_px'][0])")" -lt 1100 ] && break
    sleep 0.1
done
read -r W H < <(kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['size_px'])")
[ "$W" -lt 1100 ] || fail "window kept the fullscreen size (${W}x$H)"
grep -q "panel leaves fullscreen" "$OUT/spacewm.log" || fail "no leave log line"
[ "$(focused)" = "[]" ] || fail "Mod+Escape did not go back to walking: $(focused)"
sleep 0.4
$PX near "$(shot s8-unfullscreen)" 640 40 140 190 235 60 >/dev/null || fail "the world is not back after fullscreen"
pass "Mod+Escape leaves fullscreen: old size (${W}x$H), walking, world visible"

# ---- An app asking for fullscreen itself (like F11 / a video player).
kc close 1 >/dev/null
for _ in $(seq 30); do [ "$(kc --json list | grep -c '"id":')" -eq 0 ] && break; sleep 0.1; done
kc spawn weston-simple-egl -f >/dev/null
wait_windows 1
for _ in $(seq 30); do grep -q "panel fullscreen" <(tail -n 40 "$OUT/spacewm.log") && break; sleep 0.1; done
tail -n 60 "$OUT/spacewm.log" | grep -q "panel fullscreen" || fail "the app's fullscreen request was not honored"
pass "an app's own fullscreen request (weston-simple-egl -f) goes fullscreen"
kc input key Super+f
sleep 0.3
tail -n 30 "$OUT/spacewm.log" | grep -q "panel leaves fullscreen" || fail "Mod+F did not toggle back"
[ "$(focused)" != "[]" ] || fail "Mod+F out of fullscreen should stay in the window"
pass "Mod+F again leaves fullscreen and stays in the window"
pkill -P "$SPACE_PID" -x weston-simple-e || true
for _ in $(seq 30); do [ "$(kc --json list | grep -c '"id":')" -eq 0 ] && break; sleep 0.1; done

# ---- Mod+grave: turn to the next window and enter it when it is near.
kc mode walk
kc spawn foot >/dev/null
wait_windows 1
kc mode walk
kc spawn foot >/dev/null
wait_windows 2
kc mode walk
kc teleport 0.5 "$EYE" 0.5 --yaw 180 --pitch 0
sleep 0.3
ids=$(kc --json list | python3 -c "import json,sys; print(' '.join(str(p['id']) for p in json.load(sys.stdin)['panels']))")
kc input key Super+grave
sleep 0.3
first=$(focused)
kc mode walk
kc input key Super+grave
sleep 0.3
second=$(focused)
[ "$first" != "[]" ] && [ "$second" != "[]" ] && [ "$first" != "$second" ] ||
    fail "Mod+grave did not visit two windows (panels $ids; got $first then $second)"
pass "Mod+grave turns to the next window and enters it ($first -> $second)"

# ---- Mod+K: the key help over everything; keys don't reach the app; Escape closes.
kc mode walk
kc spawn foot >/dev/null
wait_windows 3
sleep 0.8
FOOT=$(focused)
B=$(shot s8-before-help)
kc input key Super+k
sleep 0.3
K=$(shot s8-help)
grep -q "key help shown" "$OUT/spacewm.log" || fail "Mod+K did not show the help"
python3 -c "exit(0 if $($PX diff "$B" "$K") > 0.05 else 1)" || fail "help box not drawn"
kc input type 'zzzz'
sleep 0.3
python3 -c "exit(0 if $($PX diff "$K" "$(shot s8-help-typed)") < 0.001 else 1)" ||
    fail "typing while the help is shown changed the screen (keys leaked to the app)"
kc input key Escape
sleep 0.3
tail -n 20 "$OUT/spacewm.log" | grep -q "key help closed" || fail "Escape did not close the help"
[ "$(focused)" = "$FOOT" ] || fail "closing the help changed focus: $(focused) (was $FOOT)"
python3 -c "exit(0 if $($PX diff "$B" "$(shot s8-help-closed)") < 0.01 else 1)" ||
    fail "screen not back after closing the help"
pass "Mod+K shows every key; keys don't leak to the app; Escape closes it"
kc input key Super+k
sleep 0.2
kc input click
sleep 0.2
tail -n 20 "$OUT/spacewm.log" | grep -q "key help closed" || fail "a click did not close the help"
pass "a click closes the help too"

space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "clean exit"
