#!/usr/bin/env bash
# XWayland: lazy start, X11 panels, typing, clipboard both ways, override-redirect menus
# on their parent panel, restart after the last client and after a crash.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
space_start --render 3d --log-level debug

XDISP=$(sed -n 's/.*X11 display ready (lazy) display=\(:[0-9]*\).*/\1/p' "$OUT/spacewm.log" | head -1)
[ -n "$XDISP" ] || fail "no lazy X11 display in the log"
[ -S "/tmp/.X11-unix/X${XDISP#:}" ] || fail "X11 socket missing"
pgrep -P "$SPACE_PID" -x Xwayland >/dev/null && fail "Xwayland started before any X client"
pass "X11 display $XDISP ready, Xwayland not running yet (lazy)"

# ---- First X client starts the server; it becomes a panel.
kc spawn xterm >/dev/null
wait_windows 1
grep -q "Xwayland ready" "$OUT/spacewm.log" || fail "Xwayland did not become ready"
kind=$(kc --json list | python3 -c "import json,sys; print(json.load(sys.stdin)['panels'][0]['kind'])")
[ "$kind" = x11 ] || fail "panel kind is $kind"
pass "xterm opened as an X11 panel"
[ "$(focused)" = "[1]" ] || fail "spawned xterm is not focused: $(focused)"
face_panel 1 "$PIXEL_PERFECT"
sleep 0.5

# ---- Resizing an X11 window: spacewm configures it, xterm redraws at the new size.
xsize() { kc --json list | python3 -c "import json,sys; print(*json.load(sys.stdin)['panels'][0]['size_px'])"; }
kc resize 1 600 400
for _ in $(seq 30); do read -r XW XH < <(xsize); [ "$XW" -le 620 ] && break; sleep 0.1; done
python3 -c "exit(0 if abs($XW - 600) <= 20 and abs($XH - 400) <= 30 else 1)" || fail "xterm resize gave ${XW}x$XH"
pass "spacectl resize works on an X11 window (${XW}x$XH)"
face_panel 1 "$PIXEL_PERFECT"
sleep 0.3

# ---- Typing + X11 -> Wayland clipboard: xterm runs xclip, a Wayland tool reads it.
kc input type 'printf space-from-x11 | xclip -selection clipboard'
kc input key Return
sleep 1
WL=$(space_wayland_display)
got=$(WAYLAND_DISPLAY="$WL" timeout 3 wl-paste -n 2>/dev/null || true)
[ "$got" = space-from-x11 ] || fail "Wayland paste of X11 copy gave '$got'"
pass "typing in xterm works; X11 copy -> Wayland paste"

# ---- Wayland -> X11 clipboard: wl-copy, then xclip in the (focused) xterm reads it.
WAYLAND_DISPLAY="$WL" wl-copy space-from-wayland
sleep 0.5
rm -f "$OUT/x11-paste.txt"
kc input type "xclip -o -selection clipboard > $OUT/x11-paste.txt"
kc input key Return
sleep 1
got=$(cat "$OUT/x11-paste.txt" 2>/dev/null || true)
[ "$got" = space-from-wayland ] || fail "X11 paste of Wayland copy gave '$got'"
pass "Wayland copy -> X11 paste"
shot s7-xterm >/dev/null

# ---- Override-redirect menu: a GTK combo box under X11 opens its list as an OR window.
kc spawn env GDK_BACKEND=x11 gtk3-widget-factory >/dev/null
wait_windows 2
sleep 2.5
face_panel widget-factory "$PIXEL_PERFECT"
sleep 0.5
read -r W H < <(kc --json list | python3 -c "
import json,sys
p=[p for p in json.load(sys.stdin)['panels'] if 'widget' in (p['app_id'] or '').lower()][0]
print(*p['size_px'])")
# Under X11 the window includes GTK's shadow margins (_GTK_FRAME_EXTENTS: l, r, t, b).
read -r EL ER ET EB < <(DISPLAY=$XDISP xprop -name gtk3-widget-factory _GTK_FRAME_EXTENTS \
    | sed -n 's/.*= //p' | tr -d ',' ; echo)
EL=${EL:-0}; ET=${ET:-0}
A=$(shot s7-gtk-x11)
# The "Andrea" combo box sits at (464, 250) in GTK's content coordinates.
read -r SX SY < <(python3 -c "print(640 + 464 + $EL - $W/2, 360 + 250 + $ET - $H/2)")
kc input move-to "$SX" "$SY"
kc input click
sleep 1
B=$(shot s7-gtk-x11-menu)
grep -q "X11 override-redirect window parent=" "$OUT/spacewm.log" ||
    fail "the combo list did not open as an attached override-redirect window"
D=$($PX diff "$A" "$B")
python3 -c "exit(0 if $D > 0.002 else 1)" || fail "no menu visible on the panel (diff $D)"
pass "X11 combo list drawn on its parent panel (diff $D)"
kc input key Escape
sleep 0.5

# ---- All X windows closed: Xwayland keeps running, and a client holding an X
# connection without any window (Qt apps do this) is not cut off. (Regression: stopping
# an "idle" server froze Telegram.)
kc spawn xprop -root -spy >/dev/null
kc --json list | python3 -c "import json,sys; [print(p['id']) for p in json.load(sys.stdin)['panels']]" |
    while read -r id; do kc close "$id" >/dev/null; done
sleep 4
pgrep -P "$SPACE_PID" -x Xwayland >/dev/null || fail "Xwayland stopped although an X client is connected"
pgrep -P "$SPACE_PID" -x xprop >/dev/null || fail "the windowless X client lost its server"
pass "without X windows Xwayland stays up; windowless X clients keep working"
pkill -P "$SPACE_PID" -x xprop || true

# ---- Crash: kill -9 the server; its panels go away and the next client restarts it.
XPID=$(pgrep -P "$SPACE_PID" -x Xwayland || true)
[ -n "$XPID" ] || fail "Xwayland is not a child of spacewm"
kill -9 "$XPID"
for _ in $(seq 50); do
    [ "$(kc --json list | grep -c '"id":')" -eq 0 ] && break
    sleep 0.1
done
[ "$(kc --json list | grep -c '"id":')" -eq 0 ] || fail "panels of the dead server remain"
kc spawn xeyes >/dev/null
wait_windows 1
pass "after an Xwayland crash its panels are removed and X apps work again"

space_stop || fail "unclean exit"
[ -e "/tmp/.X${XDISP#:}-lock" ] && fail "X11 lock file left behind"
[ -e "/tmp/.X11-unix/X${XDISP#:}" ] && fail "X11 socket left behind"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "clean exit, X11 lock and socket removed"
