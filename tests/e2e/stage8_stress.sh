#!/usr/bin/env bash
# Stress: many cycles of opening/closing Wayland and X11 windows while toggling
# fullscreen, inventory, help and window switching; apps killed with SIGKILL mid-way
# (also while fullscreen). spacewm must stay up, leak no file descriptors, and its memory
# must level off.
source "$(dirname "$0")/lib.sh"
CYCLES="${STRESS_CYCLES:-25}"
space_start --render 3d --log-level info
sleep 1

rss() { awk '/^VmRSS/ {print $2}' "/proc/$SPACE_PID/status"; }
fds() { ls "/proc/$SPACE_PID/fd" | wc -l; }
count() { kc --json list | python3 -c "import json,sys; print(len(json.load(sys.stdin)['panels']))"; }
wait_count() { for _ in $(seq 100); do [ "$(count)" -eq "$1" ] && return 0; sleep 0.05; done; return 1; }
close_all() {
    kc --json list | python3 -c "import json,sys; [print(p['id']) for p in json.load(sys.stdin)['panels']]" |
        while read -r id; do kc close "$id" >/dev/null 2>&1 || true; done
    wait_count 0 || { pkill -9 -P "$SPACE_PID" -x foot || true; pkill -9 -P "$SPACE_PID" -x weston-simple-s || true; wait_count 0; }
}

# Warm up once (first X11 start, caches), then measure.
kc spawn xterm >/dev/null; wait_windows 1; close_all
kc spawn foot >/dev/null; wait_windows 1; close_all
sleep 1
RSS0=$(rss); FD0=$(fds)

for i in $(seq "$CYCLES"); do
    kc mode walk
    kc spawn foot >/dev/null
    kc spawn weston-simple-shm >/dev/null
    kc spawn xterm >/dev/null
    wait_count 3 || fail "cycle $i: windows did not open ($(count))"
    # The newest window is entered: fullscreen it and back, twice.
    kc input key Super+f; sleep 0.15; kc input key Super+f; sleep 0.1
    kc input key Super+f; sleep 0.15
    # Kill one app outright while something may be fullscreen.
    if [ $((i % 3)) -eq 0 ]; then
        pkill -9 -P "$SPACE_PID" -x xterm || true
    fi
    kc input key Super+Escape
    kc input key e; kc input move-by 40 10; kc input key e
    kc input key Super+k; kc input key Escape
    kc input key Super+grave; kc input key Super+Escape
    kc input move-by 30 -5; kc input click right; kc input click
    kc input scroll 1
    close_all || fail "cycle $i: windows did not close"
    kill -0 "$SPACE_PID" 2>/dev/null || fail "spacewm died in cycle $i"
done
sleep 1.5
RSS1=$(rss); FD1=$(fds)
echo "rss ${RSS0} -> ${RSS1} kB, fds ${FD0} -> ${FD1}" >&2
[ "$FD1" -le $((FD0 + 4)) ] || fail "file descriptors leak: $FD0 -> $FD1"
pass "no fd leak after $CYCLES cycles ($FD0 -> $FD1)"
# GPU drivers keep some caches; a real leak grows with every cycle (3 windows each).
[ "$RSS1" -le $((RSS0 + 60000)) ] || fail "memory grew too much: ${RSS0} -> ${RSS1} kB"
pass "memory levels off (${RSS0} -> ${RSS1} kB)"

# Still fully working afterwards.
kc spawn foot >/dev/null
wait_windows 1
kc input type 'echo stress-ok'; kc input key Return
pass "spacewm still works after the stress run"
space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "clean exit, no panics or errors"
