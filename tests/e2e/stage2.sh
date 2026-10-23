#!/usr/bin/env bash
# Stage 2 acceptance: 3D floor + sky, mouse look (right button), WASD, 2D mode intact.
source "$(dirname "$0")/lib.sh"
PX="python3 $(dirname "$0")/pixel.py"

space_start --render 3d
sleep 0.3
A=$(shot s2-start)
# Sky at the top center, floor (green) at the bottom center.
$PX near "$A" 640 20 135 191 242 30 >/dev/null || fail "no sky at top: $($PX "$A" 640 20)"
G=$($PX "$A" 640 700)
python3 -c "r,g,b=map(int,'$G'.split()); exit(0 if g>r+30 and g>b+30 else 1)" || fail "no green floor at bottom: $G"
pass "sky above, floor below"

# Walk mode: moving the mouse right turns the camera right.
kc input move-by 400 0
sleep 0.2
B=$(shot s2-turned)
D=$($PX diff "$A" "$B")
python3 -c "exit(0 if $D > 0.05 else 1)" || fail "mouse look didn't change the view (diff $D)"
pass "mouse look turns the camera (diff $D)"

# Look straight down a lot: pitch must clamp, no flip (top of the image is floor, not sky).
kc input move-by 0 5000
sleep 0.2
C=$(shot s2-down)
T=$($PX "$C" 640 20)
python3 -c "r,g,b=map(int,'$T'.split()); exit(0 if g>b else 1)" || fail "looking down shows sky at the top (flipped?) $T"
pass "pitch clamps when looking down"
# Back to -10° pitch: (89 - 10)° = 1.379 rad = 551 px at 0.0025 rad/px.
kc input move-by 0 -551

# Walk forward with W: the view must change continuously.
sleep 0.2
E=$(shot s2-before-walk)
kc input key w --state press
sleep 0.6
kc input key w --state release
sleep 0.2
F=$(shot s2-after-walk)
D=$($PX diff "$E" "$F")
python3 -c "exit(0 if $D > 0.02 else 1)" || fail "W didn't move the camera (diff $D)"
pass "W walks (diff $D)"

# Idle: with nothing changing, two screenshots are identical (and nothing re-renders).
G1=$(shot s2-idle-a); sleep 0.5; G2=$(shot s2-idle-b)
D=$($PX diff "$G1" "$G2")
python3 -c "exit(0 if $D == 0 else 1)" || fail "idle frames differ ($D)"
pass "idle frames are stable"

# Keys are not forwarded to clients while looking, but still are otherwise.
kc spawn foot >/dev/null
wait_windows 1
space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "no errors in log"
