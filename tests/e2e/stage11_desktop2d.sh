#!/usr/bin/env bash
# The 2D desktop (Mod+D): top bar, tiling, workspaces, focus/move by keys, floating,
# fullscreen, the bar's workspace buttons; windows opened in 3D appear in 2D; going back
# to 3D gives windows their 3D sizes back.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
space_start --render 3d --log-level debug
sleep 1
info() { kc --json list | python3 -c "
import json,sys
for p in json.load(sys.stdin)['panels']:
    print(p['id'], p['workspace'], p['focused'], *(p['rect_2d'] or ['-']), *p['size_px'])"; }
rect() { info | awk -v id="$1" '$1 == id {print $4, $5, $6, $7}'; }
count_shown() { info | awk '$4 != "-"' | wc -l; }
# Terminals round their size to whole character cells: allow a few pixels.
near() { python3 -c "exit(0 if abs($1 - $2) <= ${3:-16} else 1)"; }

# Opened in 3D: it is on workspace 1 for 2D too.
kc spawn foot >/dev/null
wait_windows 1
sleep 0.8
kc mode walk
read -r _ _ _ _ _ _ _ W3D H3D < <(info)
kc input key Super+d
sleep 0.6
grep -q "render mode switched to=Flat2D" "$OUT/spacewm.log" || fail "Mod+D did not switch"
S=$(shot s11-one)
# The bar's bottom line (a color nothing else uses).
$PX near "$S" 900 29 51 59 71 8 >/dev/null || fail "no top bar: $($PX "$S" 900 29)"
read -r X Y W H < <(rect 1)
{ [ "$X $Y" = "8 38" ] && near "$W" 1264 && near "$H" 674; } || fail "one window does not fill the area under the bar: $X $Y $W $H"
pass "Mod+D: a top bar, and the 3D window fills the 2D desktop ($W x $H)"

# A second window: master left, stack right.
kc spawn foot >/dev/null
for _ in $(seq 50); do [ "$(count_shown)" -ge 2 ] && break; sleep 0.1; done
sleep 0.6
read -r X1 _ W1 _ < <(rect 1)
read -r X2 _ W2 _ < <(rect 2)
[ "$X1" = 8 ] && [ "$X2" -gt "$((X1 + W1))" ] && [ "$W1" -gt "$W2" ] || fail "no master/stack tiling: 1=($X1,$W1) 2=($X2,$W2)"
shot s11-two >/dev/null
pass "two windows tile: master left, stack right"

# Keyboard: Mod+Left focuses the master; Mod+Shift+Right swaps it to the right.
kc input key Super+Left
sleep 0.2
[ "$(info | awk '$1 == 1 {print $3}')" = True ] || fail "Mod+Left did not focus the left window: $(info)"
kc input key Super+Shift+Right
sleep 0.4
read -r X1b _ _ _ < <(rect 1)
[ "$X1b" -gt 100 ] || fail "Mod+Shift+Right did not move the window right ($X1b)"
pass "Mod+arrows move the keyboard, Mod+Shift+arrows swap windows"

# Workspaces: send window 1 to workspace 2; then show 2, then back to 1 by the bar.
kc input key Super+Shift+2
sleep 0.4
[ "$(info | awk '$1 == 1 {print $2}')" = 2 ] || fail "Mod+Shift+2 did not move it: $(info)"
[ "$(rect 1)" = "" ] || [ "$(rect 1 | awk '{print $1}')" = "-" ] || fail "window 1 still shown on workspace 1"
read -r X W < <(rect 2 | awk '{print $1, $3}')
near "$W" 1264 || fail "the window left on workspace 1 does not fill it ($W)"
kc input key Super+2
sleep 0.4
grep -q "workspace workspace=2" "$OUT/spacewm.log" || fail "Mod+2 did not switch"
[ "$(info | awk '$1 == 1 {print $4}')" = 8 ] || fail "window 1 not shown on workspace 2: $(info)"
[ "$(info | awk '$1 == 2 {print $4}')" = "-" ] || fail "window 2 still shown: $(info)"
# Bar buttons: 1 and 2 (both have windows) at x 6..34 and 36..64.
kc input move-to 20 15
kc input click
sleep 0.4
[ "$(info | awk '$1 == 2 {print $4}')" = 8 ] || fail "clicking 1 on the bar did not show workspace 1: $(info)"
pass "workspaces: Mod+Shift+N sends, Mod+N shows, the bar's buttons switch"

# Floating and fullscreen.
kc input key Super+v
sleep 0.4
read -r FX FY FW FH < <(rect 2)
[ "$FW" -lt 1000 ] && [ "$FX" -gt 100 ] || fail "Mod+V did not float it centered: $FX $FY $FW $FH"
kc input key Super+v
sleep 0.3
kc input key Super+f
sleep 0.5
read -r QX QY QW QH < <(rect 2)
{ [ "$QX $QY" = "0 0" ] && near "$QW" 1280 && near "$QH" 720; } || fail "Mod+F did not cover the screen: $(rect 2)"
F=$(shot s11-fullscreen)
$PX near "$F" 900 29 51 59 71 8 >/dev/null && fail "the bar is drawn over a fullscreen window"
kc input key Super+f
sleep 0.4
near "$(rect 2 | awk '{print $3}')" 1264 || fail "Mod+F again did not tile it back: $(rect 2)"
pass "Mod+V floats / tiles again; Mod+F covers the screen (no bar) and back"

# A window opened in 3D while workspace 2 is current shows up there in 2D.
kc input key Super+2
sleep 0.3
kc input key Super+d
sleep 0.5
grep -q "render mode switched to=Scene3D" "$OUT/spacewm.log" || fail "Mod+D did not go back to 3D"
for _ in $(seq 30); do read -r _ _ _ _ _ _ _ WB HB < <(info | awk '$1 == 2'); [ "$WB" = "$W3D" ] && break; sleep 0.1; done
[ "$WB $HB" = "$W3D $H3D" ] || fail "back in 3D the window did not get its 3D size ($WB x $HB, was $W3D x $H3D)"
kc spawn foot >/dev/null
for _ in $(seq 50); do [ "$(kc --json list | grep -o '"id":' | wc -l)" -ge 3 ] && break; sleep 0.1; done
sleep 0.6
NEW=$(info | awk '{print $1}' | sort -n | tail -1)
[ "$(info | awk -v id="$NEW" '$1 == id {print $2}')" = 2 ] || fail "a window opened in 3D did not join workspace 2: $(info)"
kc mode walk
kc input key Super+d
sleep 0.6
[ "$(info | awk -v id="$NEW" '$1 == id {print $4}')" != "-" ] || fail "it is not shown on workspace 2 in 2D: $(info)"
pass "3D sizes come back in 3D; a window opened in 3D joins the current workspace in 2D"

# X11 apps (Telegram, VS Code run like this) tile too.
before=$(count_shown)
kc spawn xterm >/dev/null
for _ in $(seq 80); do [ "$(count_shown)" -gt "$before" ] && break; sleep 0.1; done
sleep 0.8
XT=$(kc --json list | python3 -c "import json,sys; print([p['id'] for p in json.load(sys.stdin)['panels'] if p['kind']=='x11'][0])")
read -r XX XY XW XH < <(rect "$XT")
[ "$XX" != "-" ] && [ "$XX" -ge 8 ] && [ "$XY" -ge 38 ] && [ "$XW" -lt 1264 ] || fail "xterm does not tile: $XX $XY $XW $XH"
pass "an X11 window (xterm) tiles with the others ($XX,$XY ${XW}x$XH)"

space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "clean exit"
