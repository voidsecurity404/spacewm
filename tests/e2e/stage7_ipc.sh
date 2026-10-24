#!/usr/bin/env bash
# IPC: spacectl commands, event stream, shell gating, and hostile input on the socket.
source "$(dirname "$0")/lib.sh"
CFG="$OUT/config-ipc-$$.toml"
trap 'rm -f "$CFG" "$OUT/ipc-env.txt" "$OUT/watch-$$.txt"; kill %1 2>/dev/null || true; space_stop >/dev/null 2>&1 || true' EXIT
printf '[general]\nipc_allow_shell = false\n' >"$CFG"
space_start --render 3d --config "$CFG" --log-level debug

# ---- The socket: owner-only.
mode=$(stat -c %a "$SOCK")
[ "$mode" = 600 ] || fail "socket mode is $mode"
pass "socket is mode 0600"

# ---- Event stream (started first, so it sees everything below).
"$SPACECTL" --socket "$SOCK" watch >"$OUT/watch-$$.txt" 2>&1 &
sleep 0.3

kc spawn foot >/dev/null
wait_windows 1
kc list | grep -q foot || fail "list does not show foot"
pass "spacectl list shows the panel"

# ---- move / scale.
kc move 1 0 22 -4 --yaw 90
read -r x y z yaw ppu < <(kc --json list | python3 -c "
import json,sys; p=json.load(sys.stdin)['panels'][0]
print(*p['position'], round(p['yaw_deg']), p['pixels_per_unit'])")
python3 -c "exit(0 if abs($x)<1e-3 and abs($y-22)<1e-3 and abs($z+4)<1e-3 and $yaw==90 else 1)" ||
    fail "move gave $x $y $z yaw $yaw"
kc scale 1 1600
read -r ppu < <(kc --json list | python3 -c "import json,sys; print(json.load(sys.stdin)['panels'][0]['pixels_per_unit'])")
[ "${ppu%.*}" = 1600 ] || fail "scale gave $ppu"
pass "spacectl move 1 0 22 -4 and scale apply live"
kc scale 1 50 2>/dev/null && fail "out-of-range scale accepted"
kc move 1 1e9 0 0 2>/dev/null && fail "absurd position accepted"
pass "out-of-range numbers are rejected"

# ---- Window size (not panel scale): keybind and spacectl resize.
size() { kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['size_px'])"; }
read -r W0 H0 < <(size)
kc mode window 1
kc input key "Super+Ctrl+equal"
# The panel is out of view after the move above, so foot gets throttled frame callbacks
# and redraws at its new size a bit later.
for _ in $(seq 30); do read -r W1 H1 < <(size); [ "$W1" != "$W0" ] && break; sleep 0.1; done
python3 -c "exit(0 if $W1 > $W0 * 1.05 and $H1 > $H0 * 1.05 else 1)" || fail "Mod+Ctrl+= did not grow the window (${W0}x$H0 -> ${W1}x$H1)"
kc resize 1 700 450
for _ in $(seq 30); do read -r W2 H2 < <(size); [ "$W2" != "$W1" ] && break; sleep 0.1; done
python3 -c "exit(0 if abs($W2 - 700) <= 20 and abs($H2 - 450) <= 30 else 1)" || fail "spacectl resize gave ${W2}x$H2"
kc resize 1 5 5 2>/dev/null && fail "absurd size accepted"
kc mode walk
pass "window resize: Mod+Ctrl+= (${W0}x$H0 -> ${W1}x$H1), spacectl resize (-> ${W2}x$H2)"

# ---- mode.
kc mode window 1
[ "$(focused)" = "[1]" ] || fail "mode window did not focus the panel"
kc mode walk
[ "$(focused)" = "[]" ] || fail "mode walk kept focus"
kc mode window 99 2>/dev/null && fail "mode window on a missing panel succeeded"
pass "spacectl mode walk/window"

# ---- Exit code 1 on errors.
if kc focus 999 2>/dev/null; then fail "error response exited 0"; fi
pass "error responses exit with 1"

# ---- Shell commands: refused unless the config allows them; SPACEWM_SOCKET reaches apps.
kc spawn --shell "echo \$SPACEWM_SOCKET > $OUT/ipc-env.txt" 2>/dev/null && fail "shell allowed by default"
sed -i 's/ipc_allow_shell = false/ipc_allow_shell = true/' "$CFG"
kc reload-config
kc spawn --shell "echo \$SPACEWM_SOCKET > $OUT/ipc-env.txt" >/dev/null
sleep 0.5
[ "$(cat "$OUT/ipc-env.txt" 2>/dev/null)" = "$SOCK" ] || fail "SPACEWM_SOCKET not passed to apps"
pass "shell spawn needs ipc_allow_shell; apps get SPACEWM_SOCKET"

# ---- Closing produces events; check the stream.
kc close 1 >/dev/null
sleep 0.8
W=$(cat "$OUT/watch-$$.txt")
for want in "panel opened: 1" "mode: window" "mode: walk" "config reloaded" "panel closed: 1"; do
    grep -q "$want" <<<"$W" || fail "watch missed '$want':
$W"
done
pass "spacectl watch streams open/mode/config/close events"

# ---- Hostile input straight on the socket.
python3 - "$SOCK" <<'EOF' || fail "socket robustness checks failed"
import json, socket, sys, time
path = sys.argv[1]

def talk(data, timeout=2.0):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    s.connect(path)
    s.sendall(data)
    out = b""
    try:
        while not out.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            out += chunk
    except socket.timeout:
        pass
    s.close()
    return out

r = json.loads(talk(b"this is not json\n"))
assert r["response"] == "error" and "malformed" in r["message"], r
r = json.loads(talk(b'{"request":"focus_panel","id":"x"}\n'))
assert r["response"] == "error", r
r = json.loads(talk(b"x" * (70 * 1024) + b"\n"))
assert r["response"] == "error" and "too large" in r["message"], r

# 16 connections are allowed, the 17th is turned away.
held = []
for _ in range(16):
    c = socket.socket(socket.AF_UNIX); c.connect(path); held.append(c)
time.sleep(0.3)
r = json.loads(talk(b'{"request":"version"}\n'))
assert r["response"] == "error" and "too many" in r["message"], r
for c in held:
    c.close()
time.sleep(0.3)
r = json.loads(talk(b'{"request":"version"}\n'))
assert r["response"] == "version", r
print("socket checks ok")
EOF
pass "malformed, oversized and too many connections are rejected; spacewm keeps working"

space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
