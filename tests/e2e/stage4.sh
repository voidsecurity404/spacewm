#!/usr/bin/env bash
# Stage 4 acceptance: Walk/Window modes, raycast focus, cursor on the panel.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"

space_start --render 3d

# A window the user just started takes focus (Window mode) and gets the keyboard.
kc spawn foot >/dev/null
wait_windows 1
sleep 0.6
[ "$(focused)" = "[1]" ] || fail "spawned terminal not focused: $(focused)"
kc input type 'echo typed-in-window-mode'; kc input key Return
pass "spawned window takes focus"

# Mod+Escape: back to walking, nobody has the keyboard.
kc input key Super+Escape
sleep 0.2
[ "$(focused)" = "[]" ] || fail "still focused after Super+Escape: $(focused)"
pass "Super+Escape returns to Walk mode"

# Walk mode: keys never reach the client (the panel stays the same).
face_panel 1 1.0
sleep 0.3
A=$(shot s4-walk-a)
kc input type 'zzzzzz'; kc input key Return
sleep 0.4
B=$(shot s4-walk-b)
D=$($PX diff "$A" "$B")
python3 -c "exit(0 if $D == 0 else 1)" || fail "typing in Walk mode changed the panel ($D)"
pass "Walk mode keeps keys from clients"

# Looking at the panel highlights it (light blue border on the left edge at x≈350).
# At 1 m the panel shows 0.5865 screen px per window px: 994 px wide -> 583 px, so its
# left edge is at 640 - 291.5.
$PX near "$A" 350 360 115 191 255 70 >/dev/null || fail "no hover border: $($PX "$A" 350 360)"
pass "hovered panel has a highlight border"

# Left click enters it; the border turns orange and typing goes in.
kc input click
sleep 0.3
[ "$(focused)" = "[1]" ] || fail "click did not enter the panel: $(focused)"
C=$(shot s4-entered)
$PX near "$C" 350 360 255 158 46 70 >/dev/null || fail "no focus border: $($PX "$C" 350 360)"
kc input type 'echo entered-by-click'; kc input key Return
pass "left click enters the panel (orange border)"

# Mod+Escape works even while a key is held down.
# (Shift: a held key that doesn't type anything into the terminal.)
kc input key Shift_L --state press
kc input key Super+Escape
kc input key Shift_L --state release
sleep 0.2
[ "$(focused)" = "[]" ] || fail "Super+Escape failed with a key held"
pass "Super+Escape works while another key is held"

# Back sides can't be entered.
behind_panel 1 1.0
sleep 0.2
kc input click
sleep 0.2
[ "$(focused)" = "[]" ] || fail "entered a panel from behind"
pass "the back of a panel can't be entered"

# Click accuracy through the whole 3D chain: weston-clickdot draws a cyan mark where it
# received the click. Viewed at the pixel-perfect distance, that mark must appear on
# screen exactly where we clicked.
kc spawn weston-clickdot >/dev/null
wait_windows 2
sleep 0.8
face_panel clickdot "$PIXEL_PERFECT"
sleep 0.3
kc input move-to 500 300
kc input click
sleep 0.4
E=$(shot s4-clickdot)
read -r CX CY N < <($PX find "$E" 0 255 255 60)
# (The cursor arrow covers the lower right of the mark, which pulls its centroid ~1.6 px
# up even in 2D; 3 px allows for that plus sub-pixel resampling of the panel.)
python3 -c "import sys; sys.exit(0 if abs($CX-500) <= 3 and abs($CY-300) <= 3 else 1)" || fail "click landed at ($CX,$CY), wanted (500,300)"
pass "absolute click lands exactly (mark at $CX,$CY for click at 500,300)"
# Relative motion: at 1:1, moving the mouse 150 px moves the cursor 150 window px.
kc input move-by 150 80
kc input click
sleep 0.4
F=$(shot s4-clickdot-rel)
read -r CX2 CY2 N2 < <($PX find "$F" 0 255 255 60)
# clickdot keeps only the latest mark: it must be at (500+150, 300+80).
python3 -c "import sys; sys.exit(0 if abs($CX2-650) <= 3 and abs($CY2-380) <= 3 else 1)" || fail "relative click landed at ($CX2,$CY2), wanted (650,380)"
pass "relative pointer speed matches the screen (mark at $CX2,$CY2 for 650,380)"
kc input key Super+Escape

# Two terminals: focus follows what you enter.
kc input key Super+Return
wait_windows 3
sleep 0.6
[ "$(focused)" = "[3]" ] || fail "new terminal from Super+Return not focused: $(focused)"
kc input key Super+Escape
face_panel 1 1.0
sleep 0.2
kc input click
sleep 0.2
[ "$(focused)" = "[1]" ] || fail "could not switch back to terminal 1: $(focused)"
pass "focus follows: terminal 3 -> walk -> terminal 1"

# Text selection by dragging, checked through the primary selection.
WD=$(space_wayland_display)
kc input type "clear; printf 'DRAGSELECT%.0s' \$(seq 60)"; kc input key Return
sleep 0.5
face_panel 1 "$PIXEL_PERFECT"
sleep 0.2
# Prompt/output sit at the top-left of the terminal: drag across the first output line.
kc input move-to 160 70
kc input click --state press
kc input move-by 300 0
kc input click --state release
sleep 0.3
SEL=$(WAYLAND_DISPLAY=$WD timeout 2 wl-paste --primary --no-newline 2>/dev/null || true)
case "$SEL" in *DRAGSELECT*|*RAGSEL*|*SELECT*) pass "drag selects text (primary: ${SEL:0:24}...)";;
     *) shot s4-select >/dev/null; fail "drag selection gave '$SEL'";; esac

# Scroll wheel scrolls the terminal's history.
kc input type 'seq 1 300'; kc input key Return
sleep 0.4
G1=$(shot s4-scroll-a)
kc input scroll -- -10
sleep 0.4
G2=$(shot s4-scroll-b)
D=$($PX diff "$G1" "$G2")
python3 -c "exit(0 if $D > 0.002 else 1)" || fail "scrolling did not move the terminal ($D)"
pass "scroll wheel scrolls (diff $D)"

space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "no errors in log"
