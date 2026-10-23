#!/usr/bin/env bash
# Stage 5 acceptance: voxel world, physics, blocks, carrying/hanging panels, saved layout.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
LAYOUT="$OUT/layout-stage5.json"
rm -f "$LAYOUT" "$LAYOUT.bad"

space_start --render 3d --layout "$LAYOUT"
sleep 1.5

# The ground at spawn: the stone desk platform.
TOP=""
for y in $(seq 40 -1 0); do
    [ "$(kc get-block 0 "$y" 0)" != "air" ] && { TOP=$y; break; }
done
[ -n "$TOP" ] && [ "$(kc get-block 0 "$TOP" 0)" = "stone" ] || fail "no stone platform under spawn (top=$TOP)"
pass "spawned on the stone platform (top block y=$TOP)"
EYE=$(python3 -c "print($TOP + 1 + 1.62)")

# Jump: mid-air the view differs, after landing it is exactly the same again.
A=$(shot s5-ground); kc input key space; sleep 0.25; B=$(shot s5-air); sleep 1.2; C=$(shot s5-back)
python3 -c "exit(0 if $($PX diff "$A" "$B") > 0.1 else 1)" || fail "jump did not lift the camera"
python3 -c "exit(0 if $($PX diff "$A" "$C") == 0 else 1)" || fail "did not land back on the same spot"
pass "jump goes up and lands back"

# Walking forward moves the view; gravity keeps us on the ground (no flying off).
kc input key w --state press; sleep 0.6; kc input key w --state release; sleep 0.8
D=$(shot s5-walked)
python3 -c "exit(0 if $($PX diff "$A" "$D") > 0.05 else 1)" || fail "W did not walk"
pass "W walks"

# Break the platform block in front of the feet (aim 60° down), then put glass there.
kc teleport 0.5 "$EYE" 2.5 --yaw 0 --pitch -60
sleep 0.3
kc input key Super_L --state press
kc input click
kc input key Super_L --state release
sleep 0.3
[ "$(kc get-block 0 "$TOP" 1)" = "air" ] || fail "Mod+click did not break (0,$TOP,1): $(kc get-block 0 "$TOP" 1)"
pass "Mod+left click breaks the aimed block"
kc input key Super+6
kc input key Super_L --state press
kc input click right
kc input key Super_L --state release
sleep 0.3
[ "$(kc get-block 0 "$TOP" 1)" = "glass" ] || fail "Mod+right click did not place glass: $(kc get-block 0 "$TOP" 1)"
pass "Mod+right click places the selected block (glass)"
shot s5-glass >/dev/null

# Build a stone wall 3 m in front of the spawn (its face at z = -3).
for x in -2 -1 0 1 2; do for dy in 1 2 3 4; do kc set-block "$x" $((TOP + dy)) -4 stone; done; done
sleep 0.5

# Open a terminal, step back out of it, pick it up and hang it on the wall.
kc teleport 0.5 "$EYE" 0.5 --yaw 0 --pitch 0
kc spawn foot >/dev/null
wait_windows 1
sleep 0.8
kc input key Super+Escape
kc teleport 0.5 "$EYE" 1.0 --yaw 0 --pitch 0
sleep 0.3
kc input key Super+g
kc input scroll -- -15
sleep 0.4
shot s5-carry >/dev/null
kc input key Super+g
sleep 0.3
read -r PZ YAW < <(kc --json list | python3 -c "import json,sys; p=json.load(sys.stdin)['panels'][0]; print(p['position'][2], p['yaw_deg'])")
python3 -c "exit(0 if abs($PZ - (-2.98)) < 0.01 and abs($YAW) < 0.5 else 1)" || fail "panel not on the wall: z=$PZ yaw=$YAW"
pass "carried panel snaps flat onto the wall (z=$PZ, facing out)"
kc teleport 0.5 "$EYE" 1.5 --yaw 0 --pitch 0
sleep 0.3
shot s5-on-wall >/dev/null

# Saved within ~2 s.
sleep 2.5
grep -q '"app_id": "foot"' "$LAYOUT" || fail "layout file has no foot placement"
grep -q '"block": "glass"' "$LAYOUT" || fail "layout file has no block edits"
pass "layout saved (panel placement + block edits)"

# Restart: the terminal comes back on the wall, the blocks are still changed.
space_stop || fail "unclean exit"
space_start --render 3d --layout "$LAYOUT"
sleep 1
[ "$(kc get-block 0 "$TOP" 1)" = "glass" ] || fail "glass block lost after restart"
[ "$(kc get-block 0 $((TOP + 2)) -4)" = "stone" ] || fail "wall lost after restart"
pass "block edits survive a restart"
kc spawn foot >/dev/null
wait_windows 1
sleep 0.8
read -r PZ2 < <(kc --json list | python3 -c "import json,sys; print(json.load(sys.stdin)['panels'][0]['position'][2])")
python3 -c "exit(0 if abs($PZ2 - $PZ) < 0.001 else 1)" || fail "foot did not reopen on the wall (z=$PZ2, saved $PZ)"
pass "reopened terminal comes back on the wall"

# Performance: walking around at 60 fps with render distance 8.
kc input key Super+Escape
kc teleport 0.5 "$EYE" 0.5 --yaw 30 --pitch -5
kc input key Super+F3
kc input key w --state press
sleep 2.3
kc input key w --state release
FPS=$(grep "debug fps" "$OUT/spacewm.log" | grep -o 'fps=[0-9]*' | cut -d= -f2 | sort -n | tail -1)
MAX=$(grep "debug fps" "$OUT/spacewm.log" | grep -o 'max_frame_ms="[0-9.]*"' | tail -1)
[ "${FPS:-0}" -ge 55 ] || fail "only ${FPS:-?} fps while walking"
pass "walking at $FPS fps ($MAX)"

space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "no errors in log"
