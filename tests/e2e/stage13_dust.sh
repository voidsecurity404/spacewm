#!/usr/bin/env bash
# The dust map: it loads, you start on its floor, walls stop you, stairs are walked up
# without jumping, windows hang on its walls, nothing is redrawn while you stand still,
# and Escape opens the settings, where the world and its time of day change live.
# Map cell (c, r) center: x = (c - 24) * 2 + 1, z = (r - 26) * 2 + 1.
source "$(dirname "$0")/lib.sh"
cam() { kc --json camera | python3 -c "import json,sys; c=json.load(sys.stdin); print(*c['position'], c['yaw_deg'], c['pitch_deg'])"; }
st() { kc --json stats | python3 -c "import json,sys; print(json.load(sys.stdin)['stats']['$1'])"; }
near() { python3 -c "import sys; sys.exit(0 if abs($1 - ($2)) <= $3 else 1)"; }
hold() { kc input key "$1" --state press; sleep "$2"; kc input key "$1" --state release; sleep 0.3; }
# Teleporting puts you in fly mode: double-tap Space to walk (and land).
go() { kc teleport "$1" "$2" "$3" --yaw "$4" --pitch 0 >/dev/null; sleep 0.2; kc input key space; sleep 0.1; kc input key space; sleep 0.6; }

CFG="$OUT/dust-config-$$.toml"
printf '[world]\nmap = "dust"\n' >"$CFG"
space_start --render 3d --map dust --carry --config "$CFG" --log-level debug
sleep 1.5
grep -q "map built" "$OUT/spacewm.log" || fail "no map built"
grep -q "map uploaded" "$OUT/spacewm.log" || fail "map not uploaded to the GPU"
grep -q "shadow map drawn" "$OUT/spacewm.log" || fail "no shadow map"
TRI=$(sed -n 's/.*map built boxes=[0-9]* triangles=\([0-9]*\).*/\1/p' "$OUT/spacewm.log" | head -1)
[ "$TRI" -lt 20000 ] || fail "too many triangles: $TRI"
shot s13-spawn >/dev/null
read -r X Y Z YAW PITCH < <(cam)
near "$Y" 1.67 0.1 || fail "not standing on the spawn floor: eye y $Y"
pass "the dust map loads ($TRI triangles, textures, shadows); you start on its floor"

# ---- Standing still: no frames at all (the map costs nothing when nothing moves).
sleep 2.5
FPS=$(st fps)
near "$FPS" 0 0.5 || fail "frames while standing still: $FPS fps"
pass "standing still renders nothing ($FPS fps)"

# ---- Walls stop you: walk east across T spawn into its east wall (x = 22).
go -1 1.62 43 90
hold w 7
read -r X Y Z _ _ < <(cam)
near "$X" "22 - 0.3" 0.05 || fail "the wall did not stop you at x = 21.7: x $X"
pass "walls stop you (x $X)"

# ---- Stairs: from long, north up the ramp onto A site (4 steps of 0.35 m).
go 35 1.62 -13 0
hold w 3
read -r X Y Z _ _ < <(cam)
near "$Y" "1.4 + 1.62" 0.06 || fail "not up on A site (eye y $Y, z $Z)"
python3 -c "exit(0 if $Z < -24 else 1)" || fail "stopped on the stairs: z $Z"
shot s13-asite >/dev/null
pass "stairs are walked up without jumping (eye y $Y on A site)"

# ---- A window you open comes in your hand, farther than reading distance; carry it
# to the west wall of T spawn (x = -22), where it hangs flat.
kc teleport -19 1.62 45 --yaw -90 --pitch 0 >/dev/null
sleep 0.3
kc input key Super+Return
wait_windows 1
sleep 0.8
grep -q "new window in your hand" "$OUT/spacewm.log" || fail "the new window was not put in your hand"
[ "$(focused)" = "[]" ] || fail "a carried window should not take the keyboard yet: $(focused)"
D=$(sed -n 's/.*new window in your hand panel=[0-9]* distance=\([0-9.]*\).*/\1/p' "$OUT/spacewm.log" | tail -1)
python3 -c "exit(0 if 1.2 <= $D <= 3.0 else 1)" || fail "carried at $D m"
pass "a new window comes in your hand, $D m away"
kc teleport -21 1.62 45 --yaw -90 --pitch 0 >/dev/null
sleep 0.4
kc input key Super+g
sleep 0.3
read -r PX PYAW < <(kc --json list | python3 -c "import json,sys; p=json.load(sys.stdin)['panels'][0]; print(p['position'][0], p['yaw_deg'])")
near "$PX" -21.98 0.05 || fail "the window is not on the wall: x $PX"
near "$PYAW" 90 1 || fail "the window does not face out of the wall: yaw $PYAW"
shot s13-wall-window >/dev/null
pass "a carried window hangs flat on a wall (x $PX, facing east)"

# ---- Escape opens the settings; the world and the time of day change live.
kc input key Escape
sleep 0.3
grep -q "settings opened" "$OUT/spacewm.log" || fail "Escape did not open the settings"
shot s13-settings >/dev/null
row() { settings_click "$1"; sleep 1.2; }
SHADOWS0=$(grep -c "shadow map drawn" "$OUT/spacewm.log")
row "Time of day"
grep -q '^time_of_day = "afternoon"' "$CFG" || fail "time of day not saved: $(grep time_of_day "$CFG")"
[ "$(grep -c "shadow map drawn" "$OUT/spacewm.log")" -gt "$SHADOWS0" ] || fail "the sun moved but the shadows were not redrawn"
kc input key Escape
sleep 0.3
shot s13-afternoon >/dev/null
pass "Escape opens the settings; a new time of day redraws the shadows once"
kc input key Escape
sleep 0.3
row "World"
grep -q '^map = "voxel"' "$CFG" || fail "world not saved: $(grep '^map' "$CFG")"
grep -q 'world map=Voxel' "$OUT/spacewm.log" || fail "did not switch to the block world"
row "World"
grep -q '^map = "dust"' "$CFG" || fail "world not saved back"
[ "$(grep -c 'map uploaded' "$OUT/spacewm.log")" -ge 2 ] || fail "the map was not uploaded again"
kc input key Escape
sleep 0.3
read -r X Y Z _ _ < <(cam)
near "$Y" 1.67 0.1 || fail "not back at the spawn after switching worlds: y $Y"
pass "the world switches live (dust -> blocks -> dust), back at the spawn"

space_stop || fail "unclean exit"
grep -E "panicked|GL error" "$OUT/spacewm.log" && fail "errors in log"
rm -f "$CFG"
pass "clean exit"
