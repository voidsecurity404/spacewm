#!/usr/bin/env bash
# Soak: the newer features over and over on the dust map (windows coming in your hand,
# tilting, F5 views, F6 emotes, the launcher, Settings, switching worlds, fullscreen,
# apps killed mid-way). spacewm must stay up and keep answering, leak no file
# descriptors, and its memory must level off (checked at checkpoints: the second half
# may not grow like the first). SOAK_CYCLES=300 for a long run.
source "$(dirname "$0")/lib.sh"
CYCLES="${SOAK_CYCLES:-20}"
space_start --render 3d --map dust --carry --log-level debug
sleep 1.5

rss() { awk '/^VmRSS/ {print $2}' "/proc/$SPACE_PID/status"; }
fds() { ls "/proc/$SPACE_PID/fd" | wc -l; }
count() { kc --json list | python3 -c "import json,sys; print(len(json.load(sys.stdin)['panels']))"; }
wait_count() { for _ in $(seq 100); do [ "$(count)" -eq "$1" ] && return 0; sleep 0.05; done; return 1; }
close_all() {
    kc --json list | python3 -c "import json,sys; [print(p['id']) for p in json.load(sys.stdin)['panels']]" |
        while read -r id; do kc close "$id" >/dev/null 2>&1 || true; done
    wait_count 0 || { pkill -9 -P "$SPACE_PID" -x foot || true; pkill -9 -P "$SPACE_PID" -x xterm || true; wait_count 0; }
}

cycle() {
    local i=$1
    kc mode walk
    kc input key Super+Return                  # a terminal, in your hand
    kc spawn xterm >/dev/null
    wait_count 2 || fail "cycle $i: windows did not open ($(count))"
    kc input click                             # put it down
    kc input key Super+Next; kc input key Super+Prior; kc input key Super+Shift+r
    kc input key F5; kc input key F5           # behind, in front
    kc input key F6; kc input key $(( i % 9 + 1 ))
    sleep 0.3
    kc input key w --state press; sleep 0.2; kc input key w --state release
    kc input key F5                            # back to your eyes
    kc input key Super+space; kc input type "fo"; kc input key Escape
    kc input key Escape; kc input key Down; kc input key Escape   # settings
    kc mode window "$(kc --json list | python3 -c "import json,sys; print(json.load(sys.stdin)['panels'][0]['id'])")" >/dev/null 2>&1 || true
    kc input key Super+f; sleep 0.1; kc input key Super+f
    kc input key Super+Escape
    if [ $((i % 4)) -eq 0 ]; then pkill -9 -P "$SPACE_PID" -x xterm || true; fi
    close_all || fail "cycle $i: windows did not close"
    kill -0 "$SPACE_PID" 2>/dev/null || fail "spacewm died in cycle $i"
}

# Warm up (first X11 start, the character, caches), then measure.
cycle 0
sleep 1
RSS0=$(rss); FD0=$(fds)
HALF=$((CYCLES / 2))
for i in $(seq "$CYCLES"); do
    cycle "$i"
    # Now and then the other world and back (a full map upload each time).
    if [ $((i % 10)) -eq 0 ]; then
        kc input key Escape
        settings_click "World"; sleep 0.8; settings_click "World"; sleep 0.8
        kc input key Escape
    fi
    if [ "$i" -eq "$HALF" ]; then sleep 1; RSSH=$(rss); fi
done
sleep 1.5
RSS1=$(rss); FD1=$(fds)
echo "rss ${RSS0} -> ${RSSH} -> ${RSS1} kB, fds ${FD0} -> ${FD1}" >&2
[ "$FD1" -le $((FD0 + 4)) ] || fail "file descriptors leak: $FD0 -> $FD1"
pass "no fd leak after $CYCLES cycles ($FD0 -> $FD1)"
# A leak keeps growing: the second half must not grow (much) like the first.
python3 -c "
a, h, b = $RSS0, $RSSH, $RSS1
first, second = h - a, b - h
ok = b - a <= 80000 and second <= max(first, 0) + 20000
print(f'first half {first} kB, second half {second} kB')
exit(0 if ok else 1)" >&2 || fail "memory keeps growing: ${RSS0} -> ${RSSH} -> ${RSS1} kB"
pass "memory levels off (${RSS0} -> ${RSSH} -> ${RSS1} kB)"
kc version >/dev/null || fail "not answering after the soak"
grep -E "panicked|GL error" "$OUT/spacewm.log" && fail "errors in log"
space_stop || fail "unclean exit"
pass "still answering, clean exit"
