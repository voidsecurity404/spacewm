#!/usr/bin/env bash
# Layer shell (waybar, fuzzel, mako), the HUD and the built-in launcher.
#
# Runs on a private D-Bus session bus: notify-send and mako talk over D-Bus, and on the
# user's own bus the notification would land in the real desktop's notification daemon.
if [ -z "${SPACE_PRIVATE_BUS:-}" ]; then
    exec env SPACE_PRIVATE_BUS=1 dbus-run-session -- "$0" "$@"
fi
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
T="$OUT/layers-$$"
mkdir -p "$T/data/applications" "$T/waybar"
# Apps started inside spacewm (mako, fuzzel, waybar) exit with it; only the test's own
# Firefox (unique profile path) is stopped by name. Never pkill by plain program name:
# that would hit the user's real mako/fuzzel too.
trap 'space_stop >/dev/null 2>&1 || true; pkill -f "$T/ffprofile" 2>/dev/null || true;
      rm -rf "$T"' EXIT

# A launcher entry for Firefox with its own throwaway profile (never the user's, and
# --no-remote so it can't hand the window to a Firefox already running elsewhere).
cat >"$T/data/applications/spacetest-firefox.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Spacetest Firefox
Exec=firefox --no-remote --profile $T/ffprofile about:blank %u
EOF
mkdir -p "$T/ffprofile"
# A tiny waybar: one clock on a 30 px top bar.
cat >"$T/waybar/config" <<'EOF'
{ "layer": "top", "position": "top", "height": 30, "modules-left": ["clock"] }
EOF
printf 'window#waybar { background: #203040; color: white; }\n' >"$T/waybar/style.css"

XDG_DATA_HOME="$T/data" space_start --render 3d --log-level debug
hud() { grep "spacewm::render::hud" "$OUT/spacewm.log" | tail -1 | sed 's/.*text="\([^"]*\)".*/\1/'; }

# ---- HUD: clock + mode in the corner; Mod+F1 toggles it.
sleep 0.5
hud | grep -qE '^[0-9]{2}:[0-9]{2}  [A-Z][a-z]{2} [0-9]{1,2} [A-Z][a-z]{2}   Walk' || fail "HUD text wrong: $(hud)"
A=$(shot s7-hud)
kc input key "Super+F1"
sleep 0.3
B=$(shot s7-hud-off)
D=$($PX diff "$A" "$B" 0 0 400 40)
python3 -c "exit(0 if $D > 0.05 else 1)" || fail "Mod+F1 did not hide the HUD ($D)"
kc input key "Super+F1"
pass "HUD shows the clock and mode; Mod+F1 toggles it"

# ---- waybar on top; the HUD moves below its exclusive zone.
kc spawn waybar -c "$T/waybar/config" -s "$T/waybar/style.css" >/dev/null
for _ in $(seq 50); do grep -q 'new layer surface namespace="waybar"' "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q 'new layer surface namespace="waybar"' "$OUT/spacewm.log" || fail "waybar did not create a layer surface"
sleep 1.5
C=$(shot s7-waybar)
$PX near "$C" 640 5 32 48 64 25 >/dev/null || fail "no waybar color at the top: $($PX "$C" 640 5)"
# HUD background (dark) starts below the 30 px bar.
$PX near "$C" 20 45 20 22 26 30 >/dev/null || fail "HUD not below the bar: $($PX "$C" 20 45)"
pass "waybar is drawn on top of the 3D view; HUD sits below its exclusive zone"

# ---- mako + notify-send: a notification appears.
kc spawn mako >/dev/null
sleep 0.8
kc spawn notify-send spacewm-test-notification >/dev/null
for _ in $(seq 50); do grep -q 'namespace="notifications"' "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q 'namespace="notifications"' "$OUT/spacewm.log" || fail "mako notification did not appear"
sleep 0.5
shot s7-mako >/dev/null
pass "mako shows notify-send notifications"

# ---- fuzzel: exclusive keyboard; walking is suspended; typing + Enter launches.
kc spawn fuzzel >/dev/null
for _ in $(seq 50); do grep -q "layer takes the keyboard" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q "layer takes the keyboard" "$OUT/spacewm.log" || fail "fuzzel did not get the keyboard"
sleep 0.5
hud | grep -q "Overlay" || fail "HUD does not show the overlay mode: $(hud)"
kc input type "calculator"
sleep 0.5
shot s7-fuzzel >/dev/null
kc input key Return
wait_windows 1
kc list | grep -qi calculator || fail "fuzzel did not start the calculator"
grep -q "layer surface closed" "$OUT/spacewm.log" || fail "fuzzel did not close"
pass "fuzzel takes the keyboard, launches an app and gives the keyboard back"

# ---- Mod+Escape takes the keyboard back from a stuck exclusive layer.
kc spawn fuzzel >/dev/null
sleep 1
kc input key "Super+Escape"
sleep 0.3
grep -q "keyboard taken back from layer" "$OUT/spacewm.log" || fail "Mod+Escape did not dismiss the layer"
pkill -P "$SPACE_PID" -x fuzzel || true
pass "Mod+Escape frees the keyboard from an exclusive layer"

# ---- Built-in launcher: Mod+Space, type, Enter starts the app.
kc mode walk
kc input key "Super+space"
sleep 0.4
hud | grep -q "Launcher" || fail "HUD does not show the launcher mode: $(hud)"
kc input type "firefox"
sleep 0.3
shot s7-launcher-firefox >/dev/null
kc input key "Ctrl+u"
kc input type "spacetest"
sleep 0.3
shot s7-launcher >/dev/null
kc input key Return
for _ in $(seq 150); do kc list | grep -qi firefox && break; sleep 0.2; done
kc list | grep -qi firefox || fail "launcher did not start Firefox"
pass "Mod+Space launcher finds and starts Firefox"
sleep 2
shot s7-firefox >/dev/null

space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
