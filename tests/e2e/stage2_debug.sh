#!/usr/bin/env bash
# Debug overlay (Mod+F3), shader hot reload (Mod+F5), broken shader keeps the old program.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
SH="$OUT/shaders"; rm -rf "$SH"; mkdir -p "$SH"
cp "$ROOT"/src/render/shaders/voxel.* "$SH/"

space_start --render 3d --shader-dir "$SH"
kc teleport 0 30 40 --yaw 0 --pitch -10
kc input key Super+F3
kc input key w --state press
sleep 2.2
kc input key w --state release
FPS=$(grep "debug" "$OUT/spacewm.log" | grep -o 'fps=[0-9]*' | cut -d= -f2 | sort -n | tail -1)
[ -n "$FPS" ] && [ "$FPS" -ge 55 ] || fail "walking ran at ${FPS:-?} fps (want ~60)"
MS=$(grep "debug" "$OUT/spacewm.log" | grep -o 'avg_frame_ms="[0-9.]*"' | tail -1)
pass "debug overlay logs fps=$FPS ($MS) while walking"

# Edit the fragment shader: paint everything red, reload, check.
sed -i 's/frag_color = vec4(mix(c.rgb, u_fog_color \* c.a, fog), c.a);/frag_color = vec4(1.0, 0.0, 0.0, 1.0);/' "$SH/voxel.frag"
grep -q 'vec4(1.0, 0.0, 0.0, 1.0)' "$SH/voxel.frag" || fail "test could not edit voxel.frag"
kc input key Super+F5
sleep 0.3
R=$(shot s2-red)
$PX near "$R" 640 600 255 0 0 10 >/dev/null || fail "reloaded shader not used: $($PX "$R" 640 600)"
pass "Mod+F5 reloads shaders from --shader-dir"

# Break it: compile error must be logged with file and line, old program kept.
LINES=$(wc -l < "$SH/voxel.frag")
echo "this is not glsl" >> "$SH/voxel.frag"
kc input key Super+F5
sleep 0.3
grep -q "shader reload failed, keeping old program: .*voxel.frag line $((LINES + 1))" "$OUT/spacewm.log" || fail "no compile error with line number in log"
R2=$(shot s2-still-red)
$PX near "$R2" 640 600 255 0 0 10 >/dev/null || fail "broken shader replaced the working one"
pass "broken shader: error logged with line, old program kept"
space_stop || fail "unclean exit"
