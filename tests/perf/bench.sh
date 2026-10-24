#!/usr/bin/env bash
# Performance numbers for Stage 7.6 (not part of `just e2e`): idle CPU, and frame times
# with 5 busy terminals + Firefox playing a video, looking at them and looking away.
# Usage: tests/perf/bench.sh [label]   (results also appended to ~/spacewm-test/bench.txt)
if [ -z "${SPACE_PRIVATE_BUS:-}" ]; then
    exec env SPACE_PRIVATE_BUS=1 dbus-run-session -- "$0" "$@"
fi
source "$(dirname "$0")/../e2e/lib.sh"
LABEL=${1:-run}
T="$OUT/bench-$$"; mkdir -p "$T/ff"
trap 'space_stop >/dev/null 2>&1 || true; pkill -f "$T/ff" 2>/dev/null || true; rm -rf "$T"' EXIT

# A 30 s 720p test video without audio (muted autoplay is allowed) and a page for it.
ffmpeg -v error -f lavfi -i testsrc2=size=1280x720:rate=30 -t 30 -c:v libx264 -pix_fmt yuv420p "$T/v.mp4"
printf '<body style="margin:0;background:#000"><video src="v.mp4" autoplay loop muted style="width:100%%"></video>\n' >"$T/v.html"

cpu_ticks() { awk '{print $14 + $15}' "/proc/$SPACE_PID/stat"; }
cpu_percent() { # over $1 seconds
    local a b; a=$(cpu_ticks); sleep "$1"; b=$(cpu_ticks)
    python3 -c "print(round(($b - $a) / $(getconf CLK_TCK) / $1 * 100, 2))"
}
stat() { kc --json stats | python3 -c "import json,sys; s=json.load(sys.stdin)['stats']; print(*(f'{k}={s[k]:.2f}' if isinstance(s[k], float) else f'{k}={s[k]}' for k in ['fps','frame_ms_p50','frame_ms_p95','frame_ms_p99','panel_updates_per_s','chunk_uploads_per_s','gpu_memory_mib','panels','visible_panels']))"; }

space_start --render 3d --headless-size 1920x1080
sleep 4  # terrain meshing and uploads settle
idle=$(cpu_percent 10)
echo "[$LABEL] idle: cpu=${idle}% $(stat)"

for i in 1 2 3 4 5; do
    kc spawn foot sh -c "while :; do date +%T.%N; sleep 0.05; done" >/dev/null
done
kc spawn firefox --no-remote --profile "$T/ff" "file://$T/v.html" >/dev/null
for _ in $(seq 150); do [ "$(kc --json list | grep -c '"id":')" -ge 6 ] && break; sleep 0.2; done
sleep 8  # Firefox starts up, the video plays
kc mode walk
kc teleport 0 0 0 >/dev/null 2>&1 || true
# Look at the panels: they spawn in front of the starting spot.
read -r X Y Z YAW < <(kc --json list | python3 -c "
import json,sys,math
ps=json.load(sys.stdin)['panels']
x=sum(p['position'][0] for p in ps)/len(ps); y=sum(p['position'][1] for p in ps)/len(ps); z=sum(p['position'][2] for p in ps)/len(ps)
yaw=ps[0]['yaw_deg']; r=math.radians(yaw); d=4.0
print(x+math.sin(r)*d, y, z+math.cos(r)*d, -yaw)")
kc teleport "$X" "$Y" "$Z" --yaw "$YAW" --pitch 0
sleep 6
busy=$(cpu_percent 10)
echo "[$LABEL] busy, looking at 6 panels: cpu=${busy}% $(stat)"
kc teleport "$X" "$Y" "$Z" --yaw "$(python3 -c "print($YAW + 180)")" --pitch 0
sleep 6
away=$(cpu_percent 10)
echo "[$LABEL] busy, looking away: cpu=${away}% $(stat)"
space_stop
