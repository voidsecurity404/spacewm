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
# ---- Focus zoom: entering a far window glides the view to reading distance; leaving
# glides back to exactly where you stood.
space_start --render 3d --log-level debug
sleep 1
cam() { kc --json camera | python3 -c "import json,sys; d=json.load(sys.stdin); print(*d['position'], d['yaw_deg'], d['pitch_deg'])"; }
read -r X0 Y0 Z0 YAW0 PITCH0 < <(cam)
kc spawn foot >/dev/null
wait_windows 1
sleep 0.8
kc mode walk
sleep 0.5
read -r PX0 PY0 PZ0 < <(kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['position'])")
D0=$(python3 -c "import math; print(math.dist(($X0,$Y0,$Z0),($PX0,$PY0,$PZ0)))")
# Step back 3 m along the view and look a bit to the side.
read -r BX BY BZ < <(python3 -c "
import math; y=math.radians($YAW0)
print($X0 - 3*math.sin(y), $Y0, $Z0 + 3*math.cos(y))")
kc teleport "$BX" "$BY" "$BZ" --yaw "$(python3 -c "print($YAW0 + 8)")" --pitch 0
sleep 0.3
read -r X1 Y1 Z1 YAW1 PITCH1 < <(cam)
kc mode window 1
sleep 0.6
read -r X2 Y2 Z2 _ _ < <(cam)
D2=$(python3 -c "import math; print(math.dist(($X2,$Y2,$Z2),($PX0,$PY0,$PZ0)))")
python3 -c "exit(0 if abs($D2 - $D0) < 0.05 * $D0 else 1)" || fail "entering did not glide to reading distance ($D2 m, want $D0)"
grep -q "focus zoom in" "$OUT/spacewm.log" || fail "no focus zoom log"
pass "entering a far window glides to reading distance (${D2} m)"
kc mode walk
sleep 0.6
read -r X3 Y3 Z3 YAW3 _ < <(cam)
python3 -c "
import math
assert math.dist(($X3,$Y3,$Z3),($X1,$Y1,$Z1)) < 0.01, 'position'
assert abs(($YAW3 - $YAW1 + 180) % 360 - 180) < 0.5, 'yaw'" || fail "leaving did not glide back ($X3 $Y3 $Z3 yaw $YAW3; was $X1 $Y1 $Z1 yaw $YAW1)"
pass "leaving glides back to where you stood"
# ---- Window menu: right click on a window; entries do things; keys work too.
kc mode walk
kc teleport "$X0" "$Y0" "$Z0" --yaw "$YAW0" --pitch "$PITCH0"
sleep 0.4
M0=$(shot s10-before-menu)
kc input click right
sleep 0.3
grep -q "window menu opened" "$OUT/spacewm.log" || fail "right click on a window opened no menu"
M1=$(shot s10-menu)
python3 -c "exit(0 if $($PX diff "$M0" "$M1") > 0.02 else 1)" || fail "menu not drawn"
# "Show bigger" is entry 3: rows start 34 px down, 27 px each, a 7 px gap after entry 2;
# the menu's corner is 6 px right of / below the screen center.
read -r MX MY < <(python3 -c "print(640 + 6 + 60, 360 + 6 + 34 + 3*27 + 7 + 13)")
kc input move-to "$MX" "$MY"
kc input click
sleep 0.3
grep -q 'window menu panel=1 item=Bigger' "$OUT/spacewm.log" || fail "clicking 'Show bigger' did nothing"
ppu=$(kc --json list | python3 -c "import json,sys; print(json.load(sys.stdin)['panels'][0]['pixels_per_unit'])")
python3 -c "exit(0 if $ppu < 799 else 1)" || fail "panel not bigger (ppu $ppu)"
pass "right click opens the window menu; 'Show bigger' works (ppu $ppu)"
kc input click right
sleep 0.2
kc input key Escape
sleep 0.2
tail -n 20 "$OUT/spacewm.log" | grep -q "window menu closed" || fail "Escape did not close the menu"
[ "$(count)" -eq 1 ] || fail "Escape did something besides closing"
# Mod+right click while using the window opens it too; Up (wraps to "Close") + Enter.
kc mode window 1
sleep 0.5
kc input key Super_L --state press
kc input click right
kc input key Super_L --state release
sleep 0.3
[ "$(grep -c "window menu opened" "$OUT/spacewm.log")" -ge 3 ] || fail "Mod+right click in a window opened no menu"
kc input key Up
kc input key Return
for _ in $(seq 30); do [ "$(count)" -eq 0 ] && break; sleep 0.1; done
[ "$(count)" -eq 0 ] || fail "menu 'Close' (Up, Enter) did not close the window"
pass "Escape closes the menu; Mod+right click in a window; keyboard Up+Enter picks 'Close'"

# ---- Drag a window by its top edge (moves), its corner (resizes); a still click on
# the edge still enters the window.
kc spawn foot >/dev/null
wait_windows 1
sleep 0.8
kc mode walk
sleep 0.3
panel() { kc --json list | python3 -c "import json,sys; p=json.load(sys.stdin)['panels'][0]; print(*p['position'], p['yaw_deg'], *p['size_px'], p['pixels_per_unit'])"; }
# Stand 1 m in front of the window, then aim at a spot given in window pixels.
aim() {
    local AX AY AZ PYAW W H PPU
    read -r AX AY AZ PYAW W H PPU < <(panel)
    read -r CX CY CZ CYAW CPITCH < <(python3 -c "
import math
y = math.radians($PYAW)
nx, nz = math.sin(y), math.cos(y)          # the window's front
rx, rz = math.cos(y), -math.sin(y)         # its right (window +x)
u = ($1 - $W / 2) / $PPU                   # metres right of the center
v = ($H / 2 - $2) / $PPU                   # metres above it
cx, cy, cz = $AX + nx, $AY, $AZ + nz
tx, ty, tz = $AX + rx * u, $AY + v, $AZ + rz * u
dx, dy, dz = tx - cx, ty - cy, tz - cz
print(cx, cy, cz, math.degrees(math.atan2(dx, -dz)), math.degrees(math.atan2(dy, math.hypot(dx, dz))))")
    kc teleport "$CX" "$CY" "$CZ" --yaw "$CYAW" --pitch "$CPITCH"
    sleep 0.3
}
aim 500 8
shot s10-aim-top >/dev/null
grep -q 'drag hint hint="drag: move"' "$OUT/spacewm.log" || fail "no move hint on the top edge (panel $(panel), camera $(kc camera))"
read -r MX0 _ MZ0 _ < <(panel)
kc input click --state press
kc input move-by 250 0
kc input click --state release
sleep 0.4
read -r MX1 _ MZ1 _ < <(panel)
grep -q "drag started" "$OUT/spacewm.log" || fail "no drag started"
python3 -c "import math; exit(0 if math.dist(($MX0,$MZ0),($MX1,$MZ1)) > 0.3 else 1)" ||
    fail "dragging the top edge did not move the window ($MX0,$MZ0 -> $MX1,$MZ1)"
[ "$(focused)" = "[]" ] || fail "a drag must not enter the window"
pass "dragging the top edge moves the window"
aim 500 8
kc input click --state press
kc input click --state release
sleep 0.3
[ "$(focused)" != "[]" ] || fail "a still click on the edge did not enter the window"
pass "a still click on the edge still enters the window"
kc mode walk
sleep 0.5
read -r _ _ _ _ W0 H0 _ < <(panel)
aim $((W0 - 15)) $((H0 - 15))
grep -q 'drag hint hint="drag: resize"' "$OUT/spacewm.log" || fail "no resize hint on the corner"
kc input click --state press
kc input move-by 120 80
kc input move-by 120 80
kc input click --state release
for _ in $(seq 30); do read -r _ _ _ _ W1 H1 _ < <(panel); [ "$W1" -gt "$((W0 + 40))" ] && break; sleep 0.1; done
[ "$W1" -gt "$((W0 + 40))" ] && [ "$H1" -gt "$((H0 + 20))" ] || fail "dragging the corner did not resize (${W0}x$H0 -> ${W1}x$H1)"
grep -q "drag resize done" "$OUT/spacewm.log" || fail "no resize log"
pass "dragging the corner resizes the window (${W0}x$H0 -> ${W1}x$H1)"

space_stop || fail "unclean exit"

# ---- Dock: along the bottom; hold Mod for a cursor; click starts an app, or brings its
# window in front of you when it is open.
space_start --render 3d --log-level debug
sleep 1
D0=$(shot s10-dock)
# Favorites: foot, firefox, Telegram, VS Code, Files (those installed). Slot 0 is foot.
# How many apps the dock shows (from its log line), to find slot 0.
NFAV=$(grep 'spacewm::render::dock: dock items=' "$OUT/spacewm.log" | tail -1 | grep -o '("' | wc -l)
read -r SX SY < <(python3 -c "
w = 12 + $NFAV * 52
print((1280 - w) // 2 + 6 + 26, 720 - 14 - (52 + 12 + 6) + 6 + 26)")
grep -q 'spacewm::render::dock: dock items=\[("Foot", false)' "$OUT/spacewm.log" || fail "no dock with foot first: $(grep 'render::dock' "$OUT/spacewm.log" | tail -1)"
pass "the dock is shown along the bottom ($NFAV favorites)"
# Building is off by default: E opens nothing, a click on the ground breaks nothing.
kc input key e
sleep 0.2
grep -q "inventory opened" "$OUT/spacewm.log" && fail "E opened the inventory with building off"
read -r GX GY GZ _ < <(kc camera)
GROUND=$(python3 -c "import math; print(math.floor($GY - 1.62 - 0.5))")
BEFORE_B=$(kc get-block "$(python3 -c "import math; print(math.floor($GX))")" "$GROUND" "$(python3 -c "import math; print(math.floor($GZ - 1))")")
kc teleport "$GX" "$GY" "$GZ" --yaw 0 --pitch -60
sleep 0.3
kc input click
sleep 0.3
AFTER_B=$(kc get-block "$(python3 -c "import math; print(math.floor($GX))")" "$GROUND" "$(python3 -c "import math; print(math.floor($GZ - 1))")")
[ "$BEFORE_B" = "$AFTER_B" ] || fail "a click broke a block with building off ($BEFORE_B -> $AFTER_B)"
grep -q "spacewm::inventory: block selected\|spacewm::input::world_ops" "$OUT/spacewm.log" && fail "building actions ran"
pass "building is off by default (E and clicks don't touch blocks)"
kc input key Super_L --state press
kc input move-to "$SX" "$SY"
sleep 0.2
D1=$(shot s10-dock-cursor)
python3 -c "exit(0 if $($PX diff "$D0" "$D1") > 0.002 else 1)" || fail "holding Mod showed no cursor / hover"
kc input click
kc input key Super_L --state release
for _ in $(seq 50); do [ "$(count)" -ge 1 ] && break; sleep 0.1; done
[ "$(count)" -ge 1 ] || fail "clicking foot in the dock started nothing"
grep -q 'dock: launching app="Foot"' "$OUT/spacewm.log" || grep -q 'dock: launching app="foot"' "$OUT/spacewm.log" ||
    fail "no dock launch log"
pass "Mod + click on a dock app starts it"
sleep 0.8
kc mode walk
kc teleport 0.5 30 0.5 --yaw 180 --pitch 0
sleep 0.4
kc input key Super_L --state press
kc input move-to "$SX" "$SY"
kc input click
kc input key Super_L --state release
sleep 0.6
grep -q "dock: window brought here" "$OUT/spacewm.log" || fail "clicking an open app did not bring its window"
[ "$(focused)" != "[]" ] || fail "the brought window is not in use"
read -r BX BY BZ < <(kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['position'])")
python3 -c "exit(0 if $BZ > 0.5 and abs($BY - 30) < 0.5 else 1)" || fail "window not in front of the view ($BX $BY $BZ)"
pass "Mod + click on an open app brings its window in front of you"
space_stop || fail "unclean exit"

# ---- Seated mode: windows on an arc around you, no walking; back where they were.
space_start --render 3d --log-level debug
sleep 1
for _ in 1 2 3; do kc spawn foot >/dev/null; sleep 0.5; done
wait_windows 3
sleep 0.8
kc mode walk
sleep 0.3
positions() { kc --json list | python3 -c "import json,sys; print(json.dumps({p['id']: p['position'] for p in json.load(sys.stdin)['panels']}))"; }
BEFORE=$(positions)
kc input key Super+s
sleep 0.8
grep -q "seated mode on" "$OUT/spacewm.log" || fail "Mod+S did not start seated mode"
check_arc() {
    python3 - "$(kc --json camera)" "$(kc --json list)" "$1" <<'PY'
import json, math, sys
cam = json.loads(sys.argv[1])["position"]
panels = json.loads(sys.argv[2])["panels"]
want = int(sys.argv[3])
assert len(panels) == want, (len(panels), want)
d = [math.dist(cam, p["position"]) for p in panels]
assert max(d) - min(d) < 0.01, d
for p in panels:
    dx, dz = cam[0] - p["position"][0], cam[2] - p["position"][2]
    # The panel's front faces the camera: yaw = atan2(dx, dz).
    face = math.degrees(math.atan2(dx, dz))
    assert abs((p["yaw_deg"] - face + 180) % 360 - 180) < 1.0, (p["yaw_deg"], face)
print(round(d[0], 3))
PY
}
R=$(check_arc 3) || fail "windows not on an arc facing the view"
read -r C0 _ < <(kc camera)
kc input key w --state press; sleep 0.5; kc input key w --state release; sleep 0.3
read -r C1 _ < <(kc camera)
[ "$C0" = "$C1" ] || fail "W walked while seated ($C0 -> $C1)"
pass "Mod+S: windows on an arc (radius $R m) facing you; W doesn't walk"
kc spawn foot >/dev/null
wait_windows 4
sleep 0.6
# It was entered right away (focus zoom moved the view): back to the seat first.
kc mode walk
sleep 0.6
check_arc 4 >/dev/null || fail "a new window did not join the arc"
pass "a new window joins the arc"
kc input key Super+s
sleep 0.8
python3 - "$BEFORE" "$(positions)" <<'PY' || fail "windows not back where they were"
import json, math, sys
before, after = json.loads(sys.argv[1]), json.loads(sys.argv[2])
for k, pos in before.items():
    assert math.dist(pos, after[k]) < 0.01, (k, pos, after[k])
PY
kc input key w --state press; sleep 0.5; kc input key w --state release; sleep 0.3
read -r C2 _ < <(kc camera)
[ "$C2" != "$C1" ] || fail "walking not back after seated mode"
pass "Mod+S again: windows back where they were, walking works"
space_stop || fail "unclean exit"

# ---- Mod+D: a plain 2D desktop and back; windows stay where they were.
space_start --render 3d --log-level debug
sleep 1
kc spawn foot >/dev/null
wait_windows 1
sleep 0.8
kc mode walk
sleep 0.3
read -r P3 < <(kc --json list | python3 -c "import json,sys; print(json.load(sys.stdin)['panels'][0]['position'])")
kc input key Super+d
sleep 0.4
grep -q "render mode switched to=Flat2D" "$OUT/spacewm.log" || fail "Mod+D did not switch to 2D"
F=$(shot s10-2d)
$PX near "$F" 1200 650 135 190 240 40 >/dev/null && fail "still the 3D sky in 2D: $($PX "$F" 1200 650)"
# foot sits at (20, 20) on the 2D desktop: its dark background around (300, 300).
$PX near "$F" 300 300 18 18 18 20 >/dev/null || fail "foot not drawn in 2D: $($PX "$F" 300 300)"
[ "$(focused)" = "[1]" ] || fail "the window has no keyboard in 2D: $(focused)"
pass "Mod+D: a 2D desktop, the window drawn flat and focused"
# Mod+drag moves the window (no title bars to grab), Mod+right-drag resizes it.
kc input move-to 300 300
kc input key Super_L --state press
kc input click --state press
kc input move-to 500 400
kc input click --state release
kc input key Super_L --state release
sleep 0.3
M=$(shot s10-2d-moved)
$PX near "$M" 100 300 26 26 31 12 >/dev/null || fail "Mod+drag did not move the window: $($PX "$M" 100 300)"
$PX near "$M" 400 400 18 18 18 20 >/dev/null || fail "window not at its new place: $($PX "$M" 400 400)"
read -r SW0 SH0 < <(kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['size_px'])")
kc input move-to 700 500
kc input key Super_L --state press
kc input click right --state press
kc input move-to 600 450
kc input click right --state release
kc input key Super_L --state release
for _ in $(seq 30); do read -r SW1 SH1 < <(kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['size_px'])"); [ "$SW1" -lt "$((SW0 - 60))" ] && break; sleep 0.1; done
[ "$SW1" -lt "$((SW0 - 60))" ] && [ "$SH1" -lt "$((SH0 - 25))" ] || fail "Mod+right-drag did not resize (${SW0}x$SH0 -> ${SW1}x$SH1)"
pass "2D: Mod+drag moves a window, Mod+right-drag resizes it (${SW0}x$SH0 -> ${SW1}x$SH1)"
kc input key Super+d
sleep 0.5
grep -q "render mode switched to=Scene3D" "$OUT/spacewm.log" || fail "Mod+D did not switch back"
G=$(shot s10-3d-again)
$PX near "$G" 1200 100 135 190 240 40 >/dev/null || fail "no sky after switching back: $($PX "$G" 1200 100)"
read -r P3b < <(kc --json list | python3 -c "import json,sys; print(json.load(sys.stdin)['panels'][0]['position'])")
[ "$P3" = "$P3b" ] || fail "the panel moved ($P3 -> $P3b)"
[ "$(focused)" = "[]" ] || fail "back in 3D the window kept the keyboard"
pass "Mod+D again: back to the 3D world, the panel where it was"
space_stop || fail "unclean exit"

# ---- Settings box: a click changes a setting, saved to the file with its comments,
# applied at once; arrows work; Escape closes.
CFGS="$FAKE/settings.toml"
space_start --render 3d --config "$CFGS" --log-level debug
sleep 1
COMMENTS0=$(grep -c '^#' "$CFGS")
kc input key Super+comma
sleep 0.3
grep -q "settings opened" "$OUT/spacewm.log" || fail "Mod+comma did not open settings"
shot s10-settings >/dev/null
# Row 2 is the field of view: box 470x562 centered, rows from 50 px, 30 px each.
kc input move-to 640 $(( (720 - 562) / 2 + 50 + 2 * 30 + 15 ))
kc input click
sleep 0.4
grep -q '^fov = 80.0' "$CFGS" || fail "click did not save fov = 80: $(grep '^fov' "$CFGS")"
[ "$(grep -c '^#' "$CFGS")" = "$COMMENTS0" ] || fail "comments lost in the config file"
grep -q "config reloaded" "$OUT/spacewm.log" || fail "the new setting was not applied"
pass "a click in Settings saves the value (fov 80), keeps the file's comments, applies it"
kc input key Left
sleep 0.3
grep -q '^fov = 75.0' "$CFGS" || fail "Left did not step back: $(grep '^fov' "$CFGS")"
kc input key Escape
sleep 0.2
grep -q "settings closed" "$OUT/spacewm.log" || fail "Escape did not close settings"
pass "arrow keys change the selected setting; Escape closes"
space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
