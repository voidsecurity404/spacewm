#!/usr/bin/env bash
# Comfort: "Starting ..." while an app is on its way and clear errors; more sections
# are added below as features land.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
FAKE="$OUT/fake-comfort-$$"
rm -rf "$FAKE"; mkdir -p "$FAKE"
trap 'rm -rf "$FAKE"' EXIT
printf '#!/bin/sh\nsleep 2\nexec foot "$@"\n' >"$FAKE/slowfoot"
printf '#!/bin/sh\nsleep 0.6\nexit 3\n' >"$FAKE/broken"
chmod +x "$FAKE/slowfoot" "$FAKE/broken"
count() { kc --json list | python3 -c "import json,sys; print(len(json.load(sys.stdin)['panels']))"; }
start_with_terminal() {
    printf '[general]\nterminal = "%s"\n' "$1" >"$FAKE/config.toml"
    space_start --render 3d --config "$FAKE/config.toml" --log-level debug
    sleep 1
}

# ---- A slow app: "Starting slowfoot ..." until its window opens, then gone.
start_with_terminal "$FAKE/slowfoot"
A=$(shot s10-before-launch)
kc input key Super+Return
sleep 0.9
grep -q 'launching text="Starting slowfoot' "$OUT/spacewm.log" || fail "no Starting line for a slow app"
B=$(shot s10-launching)
$PX near "$B" 536 42 37 69 120 30 >/dev/null || fail "Starting line not drawn at the top: $($PX "$B" 536 42)"
for _ in $(seq 60); do [ "$(count)" -ge 1 ] && break; sleep 0.1; done
[ "$(count)" -ge 1 ] || fail "slowfoot did not open"
sleep 0.6
kc mode walk
sleep 0.3
C=$(shot s10-launched)
$PX near "$C" 536 42 37 69 120 30 >/dev/null && fail "Starting line still shown after the window opened"
pass "a slow app shows 'Starting ...' until its window opens"
space_stop || fail "unclean exit"

# ---- A fast app shows nothing (no flicker).
start_with_terminal foot
kc input key Super+Return
for _ in $(seq 50); do [ "$(count)" -ge 1 ] && break; sleep 0.1; done
sleep 0.5
grep -q 'launching text=' "$OUT/spacewm.log" && fail "a fast app flickered a Starting line"
pass "a fast app shows no Starting line"
space_stop || fail "unclean exit"

# ---- An app that fails: a plain error message; one that can't be found: too.
start_with_terminal "$FAKE/broken"
kc input key Super+Return
for _ in $(seq 30); do grep -q "app failed to start" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q 'app failed to start app="broken" code=3' "$OUT/spacewm.log" || fail "no error for an app exiting with code 3"
sleep 0.2
E=$(shot s10-failed)
$PX near "$E" 436 252 147 19 19 30 >/dev/null || fail "no red error box: $($PX "$E" 436 252)"
pass "an app exiting with an error says so (exit code 3)"
space_stop || fail "unclean exit"
printf '[[keybinds.spawn]]\nkey = "Mod+B"\ncommand = "spacewm-no-such-program"\n' >"$FAKE/config.toml"
space_start --render 3d --config "$FAKE/config.toml" --log-level debug
sleep 1
kc input key Super+b
sleep 0.5
grep -q "failed to start 'spacewm-no-such-program'" "$OUT/spacewm.log" || fail "missing program not reported"
pass "a missing program is reported on screen"
space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
