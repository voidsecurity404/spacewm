#!/usr/bin/env bash
# Apps already open OUTSIDE spacewm still open inside it: a second headless spacewm plays
# "the other desktop". Runs on a private D-Bus session bus and a throwaway HOME, so the
# user's real bus, Firefox profile and spacewm app profiles are never touched.
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

space_start --render 3d --log-level info
count() { kc --json list | python3 -c "import json,sys; print(len(json.load(sys.stdin)['panels']))"; }

# ---- A GApplication (D-Bus single instance): nautilus.
WAYLAND_DISPLAY="$WLA" nautilus --new-window >/dev/null 2>&1 &
for _ in $(seq 100); do ka list | grep -q Nautilus && break; sleep 0.1; done
ka list | grep -q Nautilus || fail "nautilus did not open on the other desktop"
kc spawn nautilus --new-window >/dev/null
for _ in $(seq 100); do [ "$(count)" -ge 1 ] && break; sleep 0.1; done
[ "$(count)" -ge 1 ] || fail "nautilus opened its window on the other desktop, not in spacewm"
grep -q "private D-Bus" "$OUT/spacewm.log" || fail "no private bus used"
shot s8-iso-nautilus >/dev/null
pass "nautilus already open elsewhere still opens in spacewm (private D-Bus)"
# Starting it again from spacewm reaches the copy in spacewm (a second window here).
kc spawn nautilus --new-window >/dev/null
for _ in $(seq 100); do [ "$(count)" -ge 2 ] && break; sleep 0.1; done
[ "$(count)" -ge 2 ] || fail "second nautilus window did not open in spacewm: $(kc list | tr "\n" "; ")"
pass "opening it again from spacewm opens a second window here"

# ---- Firefox (D-Bus remote by profile): own spacewm profile.
WAYLAND_DISPLAY="$WLA" MOZ_ENABLE_WAYLAND=1 firefox about:blank >/dev/null 2>&1 &
for _ in $(seq 300); do ka list | grep -qi firefox && break; sleep 0.1; done
ka list | grep -qi firefox || fail "firefox did not open on the other desktop"
before=$(count)
kc spawn firefox about:blank >/dev/null
for _ in $(seq 300); do kc list | grep -qi firefox && break; sleep 0.1; done
kc list | grep -qi firefox || fail "firefox opened on the other desktop, not in spacewm (count $before -> $(count))"
grep -q 'own profile app="firefox"' "$OUT/spacewm.log" || fail "firefox was not given its own profile"
[ -d "$XDG_DATA_HOME/spacewm/apps/firefox" ] || fail "profile directory missing"
[ "$(stat -c %a "$XDG_DATA_HOME/spacewm/apps/firefox")" = 700 ] || fail "profile directory is not private"
shot s8-iso-firefox >/dev/null
pass "firefox already open elsewhere opens in spacewm with its own profile (0700)"

# ---- An Electron app (VS Code: one instance per --user-data-dir, found by a socket).
if command -v code >/dev/null; then
    WAYLAND_DISPLAY="$WLA" code --ozone-platform=wayland --new-window >"$OUT/code-other.log" 2>&1 &
    for _ in $(seq 300); do ka list | grep -qi "code" && break; sleep 0.1; done
    ka list | grep -qi "code" || fail "VS Code did not open on the other desktop"
    kc spawn code >/dev/null
    for _ in $(seq 300); do kc list | grep -qi "code" && break; sleep 0.1; done
    kc list | grep -qi "code" || fail "VS Code opened on the other desktop, not in spacewm"
    grep -q 'own profile app="code"' "$OUT/spacewm.log" || fail "VS Code was not given its own profile"
    pass "VS Code already open elsewhere opens in spacewm (own --user-data-dir)"
fi

# ---- A plain program (foot) open elsewhere needs nothing: normal start, normal bus.
WAYLAND_DISPLAY="$WLA" foot >/dev/null 2>&1 &
for _ in $(seq 50); do ka list | grep -q foot && break; sleep 0.1; done
kc spawn foot >/dev/null
for _ in $(seq 50); do kc list | grep -q foot && break; sleep 0.1; done
kc list | grep -q foot || fail "foot did not open"
tail -n 5 "$OUT/spacewm.log" | grep -q 'args=\["foot"\]' || fail "foot was started differently: $(grep 'command="foot"' "$OUT/spacewm.log" | tail -1)"
pass "foot open elsewhere starts normally (it is not single-instance)"

BUS_PID=$(sed -n 's/.*private D-Bus session bus started.* pid=\([0-9]*\).*/\1/p' "$OUT/spacewm.log" | head -1)
[ -n "$BUS_PID" ] && kill -0 "$BUS_PID" 2>/dev/null || fail "private D-Bus daemon not running"
space_stop || fail "unclean exit"
sleep 0.3
kill -0 "$BUS_PID" 2>/dev/null && fail "private D-Bus daemon $BUS_PID left running"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "clean exit, private bus stopped"
