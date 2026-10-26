#!/usr/bin/env bash
# Apps already open OUTSIDE spacewm: opening one from spacewm hands the request to that
# copy (its window opens there). spacewm notices it from what the start did, whatever
# the app is, asks, closes the copy outside and opens it here. A second headless spacewm
# plays "the other desktop". Runs on a private D-Bus session bus and a throwaway HOME,
# so the user's real bus and profiles are never touched. The app it closes is a
# made-up one ("spacefake", a copy of foot under its own name) that only this test runs.
if [ -z "${SPACE_ISOLATION_INNER:-}" ]; then
    REAL_OUT="${SPACE_TEST_OUT:-$HOME/spacewm-test}"
    T=$(mktemp -d)
    # Screenshots must be inside spacewm's $HOME (an IPC safety rule), so the run keeps
    # its output in the throwaway home; it is copied out afterwards.
    trap 'mkdir -p "$REAL_OUT"; cp "$T"/home/out/*.png "$T"/home/out/*.log "$REAL_OUT"/ 2>/dev/null; rm -rf "$T"' EXIT
    mkdir -p "$T/home/out" "$T/home/.cache"
    SPACE_ISOLATION_INNER=1 HOME="$T/home" SPACE_TEST_OUT="$T/home/out" \
        XDG_DATA_HOME="$T/home/.local/share" XDG_CONFIG_HOME="$T/home/.config" \
        XDG_CACHE_HOME="$T/home/.cache" dbus-run-session -- bash "$0" 2> >(grep --line-buffered -E "^(FAIL|ok)" >&2)
    exit $?
fi
source "$(dirname "$0")/lib.sh"

# "The other desktop": its own spacewm, not started by the one under test.
SOCKA="$XDG_RUNTIME_DIR/spacewm-e2e-other-$$.sock"
"$SPACEWM" --backend headless --ipc-socket "$SOCKA" --layout "$OUT/layout-other-$$.json" \
    --config "$OUT/config-other-$$.toml" >"$OUT/spacewm-other.log" 2>&1 &
OTHER=$!
cleanup_other() {
    "$SPACECTL" --socket "$SOCKA" quit >/dev/null 2>&1 || kill "$OTHER" 2>/dev/null || true
    wait "$OTHER" 2>/dev/null || true
    rm -f "$OUT/layout-other-$$.json" "$OUT/config-other-$$.toml"
}
trap cleanup_other EXIT
for _ in $(seq 50); do [ -S "$SOCKA" ] && break; sleep 0.1; done
WLA=$(sed -n 's/.*spacewm listening on \([^ ]*\).*/\1/p' "$OUT/spacewm-other.log" | head -1)
[ -n "$WLA" ] || fail "the other spacewm did not start"
ka() { "$SPACECTL" --socket "$SOCKA" "$@"; }
count() { kc --json list | python3 -c "import json,sys; print(len(json.load(sys.stdin)['panels']))"; }
counta() { ka --json list | python3 -c "import json,sys; print(len(json.load(sys.stdin)['panels']))"; }
logged() { for _ in $(seq "${2:-60}"); do grep -q "$1" "$OUT/spacewm.log" && return 0; sleep 0.1; done; return 1; }

# ---- "spacefake": a made-up single-instance app no list in spacewm knows. Its launcher
# script hands the request to a running copy (a short-lived helper program with the
# same name exits fine, like Steam's or Chromium's second start), else runs the app.
F="$HOME/fake"
mkdir -p "$F/app" "$F/handoff"
cp "$(command -v foot)" "$F/app/spacefake"      # the app: a window
cp "$(command -v sleep)" "$F/handoff/spacefake" # "tell the running copy", exit 0
cat >"$F/spacefake-launcher" <<EOF
#!/bin/sh
if [ -f "$F/lock" ] && kill -0 "\$(cat "$F/lock")" 2>/dev/null; then
    exec "$F/handoff/spacefake" 0.4
fi
"$F/app/spacefake" &
echo \$! >"$F/lock"
wait
EOF
chmod +x "$F/spacefake-launcher"
start_outside() {
    WAYLAND_DISPLAY="$WLA" "$F/app/spacefake" >/dev/null 2>&1 &
    echo $! >"$F/lock"
    for _ in $(seq 50); do [ "$(counta)" -ge 1 ] && break; sleep 0.1; done
    [ "$(counta)" -ge 1 ] || fail "spacefake did not open on the other desktop"
    sleep 0.3
}
cfg() {
    printf '[desktop]\nrunning_elsewhere = "%s"\n[[keybinds.spawn]]\nkey = "Mod+B"\ncommand = "%s"\n[[keybinds.spawn]]\nkey = "Mod+N"\ncommand = "%s"\n' \
        "$1" "$F/spacefake-launcher" "$F/app/spacefake" >"$OUT/iso-config-$$.toml"
}

# spacefake is "installed": on PATH and with a .desktop file (in this throwaway home), so
# spacewm puts it behind its gate for programs typed in terminals.
export PATH="$F/app:$PATH"
mkdir -p "$XDG_DATA_HOME/applications"
printf '[Desktop Entry]\nType=Application\nName=Spacefake\nExec=spacefake\n' \
    >"$XDG_DATA_HOME/applications/spacefake.desktop"

cfg ask
space_start --render 3d --log-level info --config "$OUT/iso-config-$$.toml"
start_outside
OUTSIDE=$(cat "$F/lock")

# ---- Opening the very program that is open outside: it is never even started (it would
# open its window there); the question comes first.
kc input key Super+n
logged "not started, it would open there" || fail "an app open outside was started anyway"
logged "question shown" || fail "no question before starting it"
grep -q "spawned.*app/spacefake\"" "$OUT/spacewm.log" && fail "it was started although open outside"
[ "$(counta)" = 1 ] || fail "something opened on the other desktop"
kc input key Escape
logged "question answered yes=false" || fail "Escape did not cancel"
sleep 0.3
grep -q "spawned.*app/spacefake\"" "$OUT/spacewm.log" && fail "Cancel started it"
pass "an app open outside is held back before it starts: nothing opens there; Esc cancels"

# ---- A launcher script with another name: the first start goes outside (noticed,
# asked, learned) ...
kc input key Super+b
logged "the app went to its copy outside spacewm" || fail "the hand-off to the copy outside was not noticed"
logged "learned: this program hands its start to these" || fail "the hand-off was not learned"
logged "question shown" || fail "no question shown"
grep -q "pids=\[$OUTSIDE\]" "$OUT/spacewm.log" || fail "the wrong process was found: $(grep 'went to its copy' "$OUT/spacewm.log")"
shot s8-iso-question >/dev/null
kc input key Escape
for _ in $(seq 30); do [ "$(grep -c 'question answered yes=false' "$OUT/spacewm.log")" -ge 2 ] && break; sleep 0.1; done
sleep 0.5
kill -0 "$OUTSIDE" 2>/dev/null || fail "'Leave it' closed the copy outside"
[ "$(count)" = 0 ] || fail "'Leave it' opened a window here"
pass "a launcher script handing its start outside is noticed, asked about and learned"

# ... and the next start is held back before it runs.
SPAWNS=$(grep -c "spawned.*spacefake-launcher" "$OUT/spacewm.log")
kc input key Super+b
for _ in $(seq 60); do [ "$(grep -c 'question shown' "$OUT/spacewm.log")" -ge 3 ] && break; sleep 0.1; done
[ "$(grep -c 'question shown' "$OUT/spacewm.log")" -ge 3 ] || fail "no third question"
[ "$(grep -c "spawned.*spacefake-launcher" "$OUT/spacewm.log")" = "$SPAWNS" ] || fail "the learned launcher was started again"
pass "the learned launcher is held back before it starts"
kc input key Return
logged "asked an app outside spacewm to quit pid=$OUTSIDE ok=true" || fail "the copy outside was not asked to quit"
logged "closed outside spacewm; opening it here" 100 || fail "did not open it here after it closed"
for _ in $(seq 80); do [ "$(count)" -ge 1 ] && break; sleep 0.1; done
[ "$(count)" -ge 1 ] || fail "the app did not open in spacewm after moving"
kill -0 "$OUTSIDE" 2>/dev/null && fail "the copy outside is still running"
[ "$(counta)" = 0 ] || fail "its window is still on the other desktop"
shot s8-iso-moved >/dev/null
pass "Enter: closed on the other desktop, opened in spacewm"
space_stop || fail "unclean exit"

# ---- running_elsewhere = "leave": just say so.
kill "$(cat "$F/lock")" 2>/dev/null || true; sleep 0.3 # the copy this test opened in spacewm
cfg leave
space_start --render 3d --log-level info --config "$OUT/iso-config-$$.toml"
start_outside
kc input key Super+b
logged "the app went to its copy outside spacewm" || fail "not noticed (leave)"
sleep 0.4
grep -q "question shown" "$OUT/spacewm.log" && fail "asked although running_elsewhere = leave"
kill -0 "$(cat "$F/lock")" 2>/dev/null || fail "leave closed it"
pass "running_elsewhere = leave: nothing closed, no question"
space_stop || fail "unclean exit"
kill "$(cat "$F/lock")" 2>/dev/null || true # the copy this test opened outside

# (No real single-instance app here: spacewm finds copies by program name, and a real
# app the user has open, like their Nautilus, must never be closed by a test.)
cfg move
space_start --render 3d --log-level info --config "$OUT/iso-config-$$.toml"

# ---- Typed in a terminal in spacewm: the gate asks spacewm first; it is held back, and
# (running_elsewhere = move) the copy outside is closed and it opens here instead.
kill "$(cat "$F/lock")" 2>/dev/null || true
sleep 0.3
start_outside
OUTSIDE=$(cat "$F/lock")
grep -q "gate ready" "$OUT/spacewm.log" || fail "no gate"
kc spawn foot >/dev/null
for _ in $(seq 50); do [ "$(count)" -ge 1 ] && break; sleep 0.1; done
sleep 0.8
N0=$(count)
kc input type "cd /tmp && spacefake"
kc input key Return
logged "not started, it would open there" || fail "spacefake typed in a terminal was not held back"
[ "$(counta)" -le 1 ] || fail "something new opened on the other desktop"
logged "closed outside spacewm; opening it here" 100 || fail "not moved here"
for _ in $(seq 80); do [ "$(count)" -gt "$N0" ] && break; sleep 0.1; done
[ "$(count)" -gt "$N0" ] || fail "the typed program did not open in spacewm"
kill -0 "$OUTSIDE" 2>/dev/null && fail "the copy outside is still running"
[ "$(counta)" = 0 ] || fail "its window is still on the other desktop"
grep -q 'spawned.*args=\["spacefake"\]' "$OUT/spacewm.log" || fail "spacewm did not start it itself"
shot s8-iso-gate >/dev/null
pass "a program typed in a terminal in spacewm is held back by the gate, moved here (nothing opens outside)"
kill "$(cat "$F/lock")" 2>/dev/null || true

# ---- A program that is not single-instance (foot) open elsewhere: nothing to do.
WAYLAND_DISPLAY="$WLA" foot >/dev/null 2>&1 &
FOOT_OUT=$!
for _ in $(seq 50); do ka list | grep -q foot && break; sleep 0.1; done
before=$(count)
kc input key Super+Return
for _ in $(seq 50); do [ "$(count)" -gt "$before" ] && break; sleep 0.1; done
[ "$(count)" -gt "$before" ] || fail "foot did not open in spacewm"
sleep 0.5
kill -0 "$FOOT_OUT" 2>/dev/null || fail "the foot outside was closed"
pass "foot open elsewhere just opens here too (it is not single-instance), nothing closed"
kill "$FOOT_OUT" 2>/dev/null || true
space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
rm -f "$OUT/iso-config-$$.toml"
pass "clean exit"
