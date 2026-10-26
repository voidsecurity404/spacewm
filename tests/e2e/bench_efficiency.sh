#!/usr/bin/env bash
# Efficiency benchmark (not a pass/fail test): spacewm CPU use and frame times in a few
# typical situations. Prints one line per scene; run before and after an optimization.
#   SPACE_BUILD=release tests/e2e/bench_efficiency.sh
source "$(dirname "$0")/lib.sh"
space_start --render 3d
st() { kc --json stats | python3 -c "import json,sys; s=json.load(sys.stdin)['stats']; print(' '.join(str(round(s[k],2)) for k in '$1'.split()))"; }
# CPU seconds (user+sys) spacewm used, from /proc/<pid>/stat fields 14 and 15.
cpu() { awk '{print ($14+$15)/100}' "/proc/$SPACE_PID/stat"; }
measure() {
    local name=$1 secs=${2:-5} a b
    sleep 1
    a=$(cpu); sleep "$secs"; b=$(cpu)
    printf '%-34s cpu %5.1f%%  fps/p50/p95ms/upd/direct/MiB %s\n' "$name" \
        "$(python3 -c "print(($b-$a)/$secs*100)")" "$(st 'fps frame_ms_p50 frame_ms_p95 panel_updates_per_s direct_panels gpu_memory_mib')"
}
# Busy terminal (scrolling text, like a build log) in a big window, and a blinking one.
kc spawn foot -W 200x60 sh -c 'while :; do seq 1 40; sleep 0.016; done' >/dev/null
wait_windows 1
kc spawn foot >/dev/null
wait_windows 2
kc mode walk
face_panel 1 2.0;  measure "busy terminal, near (walk)"
face_panel 1 14;   measure "busy terminal, far (14 m)"
face_panel 2 2.0;  measure "idle terminal, near"
face_panel 1 2.0; kc mode window 1 >/dev/null
measure "busy terminal, window mode"
space_stop || true
