#!/usr/bin/env bash
# Config file: default written on first start, --check-config, live reload of fov and
# keybinds, a typo shows the error panel and keeps the old settings.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
CFG="$OUT/config-live-$$.toml"
trap 'rm -f "$CFG" "$OUT"/check-*.toml; space_stop >/dev/null 2>&1 || true' EXIT

# ---- --check-config: exit codes and the message format.
printf '[render]\nfov = 90\n' >"$OUT/check-ok.toml"
"$SPACEWM" --check-config --config "$OUT/check-ok.toml" >/dev/null || fail "valid config rejected"
printf '[input]\nwalk_speed = 4\nmous_sensitivity = 0.01\n' >"$OUT/check-bad.toml"
if msg=$("$SPACEWM" --check-config --config "$OUT/check-bad.toml" 2>&1); then
    fail "typo accepted"
fi
echo "$msg" | grep -q "check-bad.toml:3:1:" || fail "no line:column in: $msg"
echo "$msg" | grep -q "did you mean 'mouse_sensitivity'" || fail "no suggestion in: $msg"
pass "--check-config: exit codes, line:column and 'did you mean'"

# ---- First start writes the commented default config.
rm -f "$CFG"
space_start --render 3d --config "$CFG"
[ -f "$CFG" ] || fail "default config not written"
grep -q 'mod_key = "super"' "$CFG" || fail "default config looks wrong"
"$SPACEWM" --check-config --config "$CFG" >/dev/null || fail "written default does not validate"
pass "default config written on first start and valid"

# ---- fov: a saved edit changes the view within a second, without restart.
sleep 0.5
A=$(shot s7-fov-75)
sed -i 's/^fov = 75.0/fov = 110.0/' "$CFG"
for _ in $(seq 20); do grep -q "config reloaded" "$OUT/spacewm.log" && break; sleep 0.05; done
grep -q "config reloaded" "$OUT/spacewm.log" || fail "edit not picked up within 1 s"
sleep 0.2
B=$(shot s7-fov-110)
D=$($PX diff "$A" "$B")
python3 -c "exit(0 if $D > 0.2 else 1)" || fail "view did not change with fov (diff $D)"
pass "fov edit applied live within 1 s (diff $D)"

# ---- A typo: error panel on screen, old settings stay.
sed -i 's/^mouse_sensitivity = /mous_sensitivity = /' "$CFG"
for _ in $(seq 20); do grep -q "keeping the previous settings" "$OUT/spacewm.log" && break; sleep 0.05; done
grep -q "keeping the previous settings" "$OUT/spacewm.log" || fail "typo not reported"
grep -q "did you mean 'mouse_sensitivity'" "$OUT/spacewm.log" || fail "no suggestion in the log"
sleep 0.2
C=$(shot s7-config-error)
# The panel is dark red with white text, centered a bit above the middle.
read -r _ _ N < <($PX find "$C" 158 20 20 30)
[ "$N" -gt 20000 ] || fail "no red error panel on screen ($N red pixels)"
pass "typo shows the red error panel ($N px)"
# Old settings still active: away from the centered panel the view is still the 110
# degree one (the bottom rows of the screen).
D=$($PX diff "$B" "$C" 0 660 1280 720)
python3 -c "exit(0 if $D < 0.01 else 1)" || fail "view changed although the config was rejected ($D)"
pass "previous settings stay active"

# ---- Fixing the file clears the panel; keybinds are rebound live.
sed -i 's/^mous_sensitivity = /mouse_sensitivity = /' "$CFG"
sed -i 's/^spawn_terminal = .*/spawn_terminal = "Mod+T"/' "$CFG"
sed -i 's/^terminal = .*/terminal = "weston-terminal"/' "$CFG"
sleep 0.6
[ "$(grep -c "config reloaded" "$OUT/spacewm.log")" -ge 2 ] || fail "fixed config not reloaded"
D=$($PX diff "$B" "$(shot s7-config-fixed)")
python3 -c "exit(0 if $D < 0.01 else 1)" || fail "error panel still visible (diff $D)"
kc input key "Super+Return"
sleep 1
[ "$(kc --json list | grep -c '"id":')" -eq 0 ] || fail "old keybind still spawns"
kc input key "Super+t"
wait_windows 1
kc list | grep -q weston || fail "new keybind did not start the configured terminal"
pass "fixed config clears the panel; spawn_terminal rebound to Mod+T live"

# ---- render.max_fps_idle: an always-animating app is redrawn at the capped rate while
# the camera is still (fps from the Mod+F3 debug log, one line per second).
fps() { grep "spacewm::render::view: debug" "$OUT/spacewm.log" | tail -2 | head -1 | sed 's/.*fps=\([0-9]*\).*/\1/'; }
kc spawn weston-simple-egl >/dev/null
wait_windows 2
kc input key "Super+F3"
sleep 3
uncapped=$(fps)
sed -i 's/^max_fps_idle = .*/max_fps_idle = 5/' "$CFG"
sleep 3.5
capped=$(fps)
kc input key "Super+F3"
python3 -c "exit(0 if $uncapped >= 30 and 3 <= $capped <= 7 else 1)" ||
    fail "max_fps_idle: uncapped $uncapped fps, capped $capped fps"
pass "max_fps_idle = 5 caps app-driven redraws ($uncapped -> $capped fps)"

space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
