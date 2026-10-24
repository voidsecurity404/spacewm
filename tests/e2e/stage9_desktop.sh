#!/usr/bin/env bash
# Everyday desktop: media keys with an OSD (fake tools: the user's real volume and
# brightness are never touched), media keys on the lock screen, Print / Shift+Print,
# Mod+L, auto-lock after idle, the HUD status, reopening last session's apps.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
FAKE="$OUT/fake-desktop-$$"
rm -rf "$FAKE"; mkdir -p "$FAKE/shots"
trap 'rm -rf "$FAKE"' EXIT
cat >"$FAKE/level.sh" <<SH
#!/bin/sh
# level.sh <name> up|down|mute|get: a fake volume/brightness control.
f="$FAKE/\$1"; [ -f "\$f" ] || echo "50 0" >"\$f"
read -r v m <"\$f"
case "\$2" in
    up) v=\$((v + 5)) ;;
    down) v=\$((v - 5)) ;;
    mute) m=\$((1 - m)) ;;
    get) [ "\$m" = 1 ] && echo "Volume: \$v% [MUTED]" || echo "Volume: \$v%"; exit 0 ;;
esac
echo "\$v \$m" >"\$f"
SH
chmod +x "$FAKE/level.sh"
LOCKER="$ROOT/target/debug/examples/test_locker"
CFG="$FAKE/config.toml"
cat >"$CFG" <<TOML
[desktop]
lock_command = "$LOCKER 00ff00 2.5"
lock_after_minutes = 1
screenshot_dir = "$FAKE/shots"
region_selector = "echo 10,10 200x100"
volume_up = "$FAKE/level.sh vol up"
volume_down = "$FAKE/level.sh vol down"
volume_mute = "$FAKE/level.sh vol mute"
volume_get = "$FAKE/level.sh vol get"
brightness_up = "$FAKE/level.sh light up"
brightness_down = "$FAKE/level.sh light down"
brightness_get = "$FAKE/level.sh light get"
TOML
LAYOUT="$FAKE/layout.json"
space_start --render 3d --config "$CFG" --layout "$LAYOUT" --log-level debug
sleep 1

# ---- Media keys: the tool runs, the OSD shows the new level.
A=$(shot s9-before)
kc input key XF86AudioRaiseVolume
for _ in $(seq 40); do grep -q 'osd what="Volume" percent=Some(55)' "$OUT/spacewm.log" && break; sleep 0.05; done
grep -q 'osd what="Volume" percent=Some(55) muted=false' "$OUT/spacewm.log" || fail "volume up: no OSD at 55%"
sleep 0.1
B=$(shot s9-osd)
python3 -c "exit(0 if $($PX diff "$A" "$B") > 0.004 else 1)" || fail "OSD not drawn"
kc input key XF86AudioMute
for _ in $(seq 40); do grep -q 'muted=true' "$OUT/spacewm.log" && break; sleep 0.05; done
grep -q 'osd what="Volume" percent=Some(55) muted=true' "$OUT/spacewm.log" || fail "mute: no muted OSD"
kc input key XF86MonBrightnessDown
for _ in $(seq 40); do grep -q 'osd what="Brightness" percent=Some(45)' "$OUT/spacewm.log" && break; sleep 0.05; done
grep -q 'osd what="Brightness" percent=Some(45)' "$OUT/spacewm.log" || fail "brightness down: no OSD at 45%"
sleep 1.8
python3 -c "exit(0 if $($PX diff "$A" "$(shot s9-osd-gone)") < 0.002 else 1)" || fail "OSD did not go away"
pass "media keys run the tools and show the level (OSD), then it fades"

# ---- HUD: date and battery (when the machine has one).
grep -q "hud text=.*[A-Z][a-z][a-z] [0-9]* [A-Z][a-z][a-z]" "$OUT/spacewm.log" || fail "HUD has no date"
if ls /sys/class/power_supply/*/type 2>/dev/null | xargs grep -lx Battery >/dev/null 2>&1; then
    grep -q "hud text=.*Bat [0-9]*%" "$OUT/spacewm.log" || fail "HUD has no battery"
fi
pass "HUD shows date (and battery)"

# ---- Screenshots: Print = whole screen, Shift+Print = the selected region.
kc input key Print
for _ in $(seq 60); do ls "$FAKE"/shots/*.png >/dev/null 2>&1 && break; sleep 0.1; done
FULL=$(ls "$FAKE"/shots/*.png 2>/dev/null | head -1)
[ -n "$FULL" ] || fail "Print saved no screenshot"
size() { python3 -c "import struct,sys; d=open(sys.argv[1],'rb').read(24); print(*struct.unpack('>II', d[16:24]))" "$1"; }
[ "$(size "$FULL")" = "1280 720" ] || fail "full screenshot is $(size "$FULL")"
for _ in $(seq 50); do grep -q "spacewm::desktop: screenshot saved" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q "spacewm::desktop: screenshot saved.*copied=true" "$OUT/spacewm.log" ||
    fail "screenshot not copied to the clipboard: $(grep 'desktop: screenshot' "$OUT/spacewm.log")"
got=$(WAYLAND_DISPLAY="$(space_wayland_display)" timeout 3 wl-paste --list-types 2>/dev/null || true)
grep -q image/png <<<"$got" || fail "clipboard has no image/png ($got)"
sleep 1.1   # file names have seconds
kc input key Shift+Print
for _ in $(seq 60); do [ "$(ls "$FAKE"/shots/*.png | wc -l)" -ge 2 ] && break; sleep 0.1; done
REGION=$(ls -t "$FAKE"/shots/*.png | head -1)
[ "$REGION" != "$FULL" ] || fail "Shift+Print saved nothing"
[ "$(size "$REGION")" = "200 100" ] || fail "region screenshot is $(size "$REGION")"
pass "Print saves the screen (and copies it), Shift+Print a region"

# ---- Mod+L locks; media keys still work while locked; keybinds don't.
kc input key Super+l
for _ in $(seq 50); do grep -q "lock confirmed" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q "lock confirmed" "$OUT/spacewm.log" || fail "Mod+L did not lock"
kc input key XF86AudioRaiseVolume
for _ in $(seq 40); do grep -q 'percent=Some(60)' "$OUT/spacewm.log" && break; sleep 0.05; done
grep -q 'osd what="Volume" percent=Some(60)' "$OUT/spacewm.log" || fail "volume key did nothing while locked"
for _ in $(seq 60); do grep -q "session unlocked" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q "session unlocked" "$OUT/spacewm.log" || fail "test locker did not unlock"
pass "Mod+L locks; media keys work on the lock screen"

# ---- Reopen last session's apps.
kc spawn foot >/dev/null
kc spawn xterm >/dev/null
wait_windows 2
sleep 1
space_stop || fail "unclean exit"
python3 - "$LAYOUT" <<'PY' || fail "layout file does not list the open apps"
import json, sys
apps = json.load(open(sys.argv[1]))["apps"]
assert any(a.endswith("/foot") for a in apps) and any(a.endswith("/xterm") for a in apps), apps
PY
space_start --render 3d --config "$CFG" --layout "$LAYOUT" --log-level debug
for _ in $(seq 100); do [ "$(kc list | grep -cE 'foot|xterm|XTerm')" -ge 2 ] && break; sleep 0.1; done
[ "$(kc list | grep -cE 'foot|XTerm|xterm')" -ge 2 ] || fail "apps not reopened: $(kc list)"
grep -q "reopening last session's apps count=2" "$OUT/spacewm.log" || fail "no reopen log"
pass "foot and xterm come back after a restart"
kc --json list | python3 -c "import json,sys; [print(p['id']) for p in json.load(sys.stdin)['panels']]" |
    while read -r id; do kc close "$id" >/dev/null; done

# ---- Auto-lock after 1 minute without input.
echo "waiting ~75 s for auto-lock..." >&2
for _ in $(seq 90); do grep -q "auto-lock" "$OUT/spacewm.log" && break; sleep 1; done
grep -q "no input for a while: auto-lock" "$OUT/spacewm.log" || fail "no auto-lock after 1 minute"
for _ in $(seq 30); do grep -q "lock confirmed" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q "lock confirmed" "$OUT/spacewm.log" || fail "auto-lock did not lock"
pass "auto-lock after a minute without input"
for _ in $(seq 60); do grep -q "session unlocked" "$OUT/spacewm.log" && break; sleep 0.1; done

space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "clean exit"
