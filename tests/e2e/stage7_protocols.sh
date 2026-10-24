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

space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
