#!/usr/bin/env bash
# Stage 7.5 desktop protocols, each checked with the tool people use it with.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"
space_start --render 2d --log-level debug
WL=$(space_wayland_display)
globals=$(WAYLAND_DISPLAY="$WL" wayland-info 2>/dev/null | sed -n "s/.*interface: '\([^']*\)'.*/\1/p")
has() { grep -qx "$1" <<<"$globals" || fail "global $1 missing"; }

# ---- xdg-decoration: server-side, so foot draws no title bar of its own.
has zxdg_decoration_manager_v1
kc spawn foot >/dev/null
wait_windows 1
sleep 1
S=$(shot s7-ssd-foot)
# foot sits at (20, 20) in 2D. Its CSD title bar would be a light strip at the top; with
# server-side decorations the first rows are the terminal's dark background.
$PX near "$S" 700 26 20 20 20 40 >/dev/null || fail "foot still draws a title bar: $($PX "$S" 700 26)"
pass "xdg-decoration: foot uses server-side mode (no title bar)"
kc close 1 >/dev/null

space_stop || fail "unclean exit"

# ---- ext-image-copy-capture: grim and wf-recorder capture the composited 3D frame.
space_start --render 3d --log-level debug
WL=$(space_wayland_display)
globals=$(WAYLAND_DISPLAY="$WL" wayland-info 2>/dev/null | sed -n "s/.*interface: '\([^']*\)'.*/\1/p")
has ext_image_copy_capture_manager_v1
has ext_output_image_capture_source_manager_v1
sleep 0.5
rm -f "$OUT/grim.png" "$OUT/grim-region.png" "$OUT/rec.mp4"
WAYLAND_DISPLAY="$WL" timeout 10 grim "$OUT/grim.png" || fail "grim failed"
REF=$(shot s7-grim-ref)
D=$($PX diff "$OUT/grim.png" "$REF")
python3 -c "exit(0 if $D < 0.02 else 1)" || fail "grim image differs from the screen ($D)"
pass "grim captures the 3D view (diff to spacewm's own screenshot $D)"
WAYLAND_DISPLAY="$WL" timeout 10 grim -g "100,80 400x300" "$OUT/grim-region.png" || fail "grim -g failed"
python3 -c "
import sys; sys.path.insert(0, '$(dirname "$0")')
exec(open('$(dirname "$0")/pixel.py').read().replace('\nmain()\n', '\n'))
w, h, _, _ = read_png('$OUT/grim-region.png'); assert (w, h) == (400, 300), (w, h)" ||
    fail "grim -g gave the wrong size"
pass "grim -g captures a region"
# Something must move: wf-recorder asks for frames "with damage", i.e. only when the
# screen changes.
kc spawn weston-simple-egl >/dev/null
wait_windows 1
WAYLAND_DISPLAY="$WL" timeout -s INT 3 wf-recorder -f "$OUT/rec.mp4" >/dev/null 2>&1 || true
size=$(stat -c %s "$OUT/rec.mp4" 2>/dev/null || echo 0)
[ "$size" -gt 10000 ] || fail "wf-recorder produced no video ($size bytes)"
frames=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames \
    -of csv=p=0 "$OUT/rec.mp4" 2>/dev/null || echo 0)
[ "${frames:-0}" -gt 20 ] || fail "wf-recorder recorded only ${frames:-0} frames"
pass "wf-recorder records the screen ($frames frames, $size bytes)"

# ---- Small protocols: present, and pointer-constraints honored on a panel.
for g in wp_fractional_scale_manager_v1 zwp_relative_pointer_manager_v1 \
    zwp_pointer_constraints_v1 wp_cursor_shape_manager_v1 xdg_activation_v1 \
    zwp_primary_selection_device_manager_v1 wp_presentation; do
    has "$g"
done
pass "fractional-scale, relative-pointer, pointer-constraints, cursor-shape, activation, primary selection, presentation-time globals"
kc spawn weston-constraints >/dev/null
for _ in $(seq 50); do kc list | grep -q constraints && break; sleep 0.1; done
CID=$(kc --json list | python3 -c "import json,sys; print([p['id'] for p in json.load(sys.stdin)['panels'] if 'constraints' in (p['app_id'] or '') or 'constraints' in (p['title'] or '')][0])")
face_panel "$CID" "$PIXEL_PERFECT"
kc mode window "$CID"
sleep 0.3
kc input click; sleep 0.2   # unconstrained -> confined
kc input click; sleep 0.2   # confined -> locked
kc input move-by 60 0
sleep 0.2
grep -q "pointer locked by the app" "$OUT/spacewm.log" || fail "pointer lock not honored"
pass "pointer-constraints: a locked pointer freezes the panel cursor"
kc close "$CID" >/dev/null
kc mode walk

# ---- presentation-time: weston-presentation-shm gets "presented" feedback every frame.
n=$(WAYLAND_DISPLAY="$WL" timeout -s INT 2 env -u DISPLAY weston-presentation-shm -f 2>&1 | grep -c "p2p" || true)
[ "$n" -gt 60 ] || fail "presentation feedback for only $n frames in 2 s"
pass "presentation-time: $n frames reported presented in 2 s"

# ---- xdg-activation: an app asking for attention pulses instead of taking focus.
kc spawn foot -o bell.urgent=yes sh -c "sleep 1.5; printf '\\a'; sleep 30" >/dev/null
for _ in $(seq 50); do kc list | grep -q foot && break; sleep 0.1; done
kc mode walk
sleep 2.5
grep -q "panel asks for attention" "$OUT/spacewm.log" || fail "foot's urgent bell did not request activation"
[ "$(focused)" = "[]" ] || fail "activation stole focus: $(focused)"
P1=$(shot s7-attention-1); sleep 0.15; P2=$(shot s7-attention-2)
D=$($PX diff "$P1" "$P2")
python3 -c "exit(0 if $D > 0.0005 else 1)" || fail "no pulsing border ($D)"
pass "xdg-activation: the panel pulses, focus stays where it was"
kc close "$(kc --json list | python3 -c "import json,sys; print([p['id'] for p in json.load(sys.stdin)['panels'] if p['app_id']=='foot'][-1])")" >/dev/null

# ---- primary selection: select-to-copy / middle-click paste between apps.
WAYLAND_DISPLAY="$WL" wl-copy --primary space-primary
sleep 0.3
got=$(WAYLAND_DISPLAY="$WL" timeout 3 wl-paste --primary -n 2>/dev/null || true)
[ "$got" = space-primary ] || fail "primary selection gave '$got'"
pass "primary selection round trip"

# ---- ext-idle-notify + idle-inhibit: swayidle fires after 2 s without input, resumes
# on input, and doesn't fire while mpv plays a video (it holds an idle inhibitor).
has ext_idle_notifier_v1
has zwp_idle_inhibit_manager_v1
IDLE="$OUT/idle-$$"
rm -f "$IDLE".*
kc spawn swayidle -w timeout 2 "touch $IDLE.idle" resume "touch $IDLE.resume" >/dev/null
sleep 3.2
[ -e "$IDLE.idle" ] || fail "swayidle did not see the session go idle"
kc input move-by 5 0
sleep 0.5
[ -e "$IDLE.resume" ] || fail "input did not end the idle state"
pass "swayidle: idle after 2 s without input, resumed by input"
rm -f "$IDLE".*
kc spawn mpv --really-quiet --loop --geometry=320x240 av://lavfi:testsrc=size=320x240:rate=30 >/dev/null
for _ in $(seq 80); do grep -q "idle inhibited" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q "idle inhibited" "$OUT/spacewm.log" || fail "mpv did not inhibit idle"
kc input move-by 5 0
sleep 3.2
[ -e "$IDLE.idle" ] && fail "swayidle fired although mpv inhibits idle"
pass "mpv's idle inhibitor keeps the session awake"
has wp_viewporter
kc list | grep -q mpv || fail "mpv has no window (it needs wp_viewporter)"
pass "wp_viewporter: mpv shows its video through a viewport"
pkill -P "$SPACE_PID" -x mpv || true
pkill -P "$SPACE_PID" -x swayidle || true
rm -f "$IDLE".*

# ---- ext-session-lock with a test locker: lock, unlock, focus comes back; a second
# locker is refused while the first one holds the lock.
has ext_session_lock_manager_v1
LOCKER="$ROOT/target/debug/examples/test_locker"
kc focus "$(kc --json list | python3 -c "import json,sys; print(json.load(sys.stdin)['panels'][0]['id'])")"
before=$(focused)
[ "$before" != "[]" ] || fail "expected a focused panel before locking"
WAYLAND_DISPLAY="$WL" "$LOCKER" 00ff00 2.5 >/dev/null &
LPID=$!
for _ in $(seq 50); do grep -q "lock confirmed" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q "lock confirmed" "$OUT/spacewm.log" || fail "test locker did not lock"
sleep 0.3
$PX near "$(shot s7-test-locked)" 640 360 0 255 0 20 >/dev/null || fail "lock surface not shown"
[ "$(focused)" = "[]" ] || fail "a window kept keyboard focus while locked: $(focused)"
if WAYLAND_DISPLAY="$WL" "$LOCKER" ff0000 0.1 >/dev/null 2>&1; then
    fail "a second locker was allowed while the session is locked"
fi
pass "lock: only the locker is shown and has the keyboard; a second locker is refused"
wait "$LPID" || fail "test locker failed"
sleep 0.3
grep -q "session unlocked" "$OUT/spacewm.log" || fail "unlock not seen"
[ "$(focused)" = "$before" ] || fail "focus not restored after unlock: $(focused) (was $before)"
U=$(shot s7-unlocked)
$PX near "$U" 640 360 0 255 0 20 >/dev/null && fail "lock surface still shown after unlock"
pass "unlock: the desktop and the focused panel come back"

# ---- swaylock hides everything; killing it keeps the screen locked.
kc spawn swaylock -c ff00ff >/dev/null
for _ in $(seq 50); do grep -q "lock confirmed" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q "lock confirmed" "$OUT/spacewm.log" || fail "swaylock did not lock"
sleep 0.5
L=$(shot s7-locked)
$PX near "$L" 640 600 255 0 255 30 >/dev/null || fail "lock screen is not swaylock's color: $($PX "$L" 640 600)"
$PX near "$L" 300 100 255 0 255 30 >/dev/null || fail "something besides the lock is visible"
pass "swaylock: only the lock surface is shown"
WAYLAND_DISPLAY="$WL" timeout 5 grim "$OUT/grim-locked.png" 2>/dev/null && fail "grim captured the locked screen"
pass "screen capture is refused while locked"
kc input key "Super+Return"
sleep 0.5
grep -q 'spawned.*command="foot"' "$OUT/spacewm.log" && fail "keybind worked while locked"
pass "keybinds are off while locked"
pkill -P "$SPACE_PID" -x swaylock
sleep 0.8
R=$(shot s7-lock-crashed)
$PX near "$R" 300 100 140 0 0 25 >/dev/null || fail "after killing swaylock the screen is not red: $($PX "$R" 300 100)"
pass "pkill swaylock: the session stays locked (red screen)"
kc spawn swaylock -c 00ffff >/dev/null
for _ in $(seq 50); do grep -q "new locker replaces" "$OUT/spacewm.log" && break; sleep 0.1; done
grep -q "new locker replaces" "$OUT/spacewm.log" || fail "a new locker was not accepted"
sleep 0.5
$PX near "$(shot s7-relocked)" 640 600 0 255 255 30 >/dev/null || fail "new locker not shown"
pass "a new locker takes over the abandoned lock"
pkill -P "$SPACE_PID" -x swaylock || true

space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
