#!/usr/bin/env bash
# Performance behavior: panels out of view cost nothing, far panels use half-resolution
# textures, spacectl stats reports it all.
source "$(dirname "$0")/lib.sh"
space_start --render 3d
st() { kc --json stats | python3 -c "import json,sys; print(json.load(sys.stdin)['stats']['$1'])"; }
for i in 1 2; do kc spawn foot sh -c "while :; do date +%T.%N; sleep 0.05; done" >/dev/null; done
wait_windows 2
kc mode walk
face_panel 1 2.0
sleep 2.5
[ "$(st visible_panels)" -ge 1 ] || fail "no visible panels while looking at them"
up=$(st panel_updates_per_s)
python3 -c "exit(0 if $up >= 10 else 1)" || fail "busy terminals not re-rendered ($up/s)"
near_mem=$(st gpu_memory_mib)
pass "looking at busy terminals: $up panel updates/s, $(st fps) fps"

# Turn around: nothing visible changes, so nothing is rendered or re-textured.
read -r X Y Z YAW < <(kc --json list | python3 -c "
import json,sys,math; p=json.load(sys.stdin)['panels'][0]; r=math.radians(p['yaw_deg'])
print(p['position'][0]+math.sin(r)*2, p['position'][1], p['position'][2]+math.cos(r)*2, 180-p['yaw_deg'])")
kc teleport "$X" "$Y" "$Z" --yaw "$YAW" --pitch 0
sleep 3
[ "$(st visible_panels)" = 0 ] || fail "panels still counted visible when behind the camera"
python3 -c "exit(0 if $(st panel_updates_per_s) == 0 and $(st fps) <= 2 else 1)" ||
    fail "work while looking away: $(st panel_updates_per_s) updates/s, $(st fps) fps"
pass "looking away: no panel updates, no frames (apps get throttled frame callbacks)"

# Far away (> 10 m): half-resolution textures, a quarter of the memory.
face_panel 1 14
sleep 2
far_mem=$(st gpu_memory_mib)
python3 -c "exit(0 if $far_mem < $near_mem - 1 else 1)" || fail "no LOD: $near_mem MiB near, $far_mem MiB far"
face_panel 1 2.0
sleep 2
back_mem=$(st gpu_memory_mib)
python3 -c "exit(0 if abs($back_mem - $near_mem) < 0.5 else 1)" || fail "full resolution not restored ($back_mem vs $near_mem MiB)"
pass "far panels use half-resolution textures ($near_mem -> $far_mem MiB) and come back sharp"

kc stats | grep -q "frame time" || fail "spacectl stats output"
pass "spacectl stats: $(kc stats | sed -n 2p | tr -s ' ')"
space_stop || fail "unclean exit"
pass "clean exit"
