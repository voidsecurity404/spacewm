#!/usr/bin/env bash
# A GPU app (dmabuf buffers, like most real apps) up close: its texture is drawn directly,
# without a copy, and spacewm stays clean. The client animates, so its picture can't be
# compared pixel by pixel with the copy; the orientation is checked by eye.
source "$(dirname "$0")/lib.sh"
trap 'space_stop >/dev/null 2>&1 || true' EXIT
st() { kc --json stats | python3 -c "import json,sys; print(json.load(sys.stdin)['stats']['$1'])"; }

command -v weston-simple-egl >/dev/null || { echo "skip - weston-simple-egl not installed"; exit 0; }

space_start --render 3d
kc spawn weston-simple-egl >/dev/null
wait_windows 1
sleep 1.5
face_panel 1 "$PIXEL_PERFECT"
sleep 1
[ "$(st direct_panels)" = 1 ] || fail "GPU window not drawn directly (direct_panels=$(st direct_panels))"
shot s17-dmabuf >/dev/null
pass "GPU window drawn directly from its own texture (screenshot: $OUT/s17-dmabuf.png)"

space_stop || fail "unclean exit"
grep -E "panicked" "$OUT/spacewm.log" && fail "panic in log"
pass "clean exit"
