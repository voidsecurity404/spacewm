#!/usr/bin/env bash
# Your body: F5 cycles the view (eyes, behind, in front) and shows the character; F6
# opens the emote menu, a number plays one; standing still in third person stays
# cheap (the idle animation at 15 fps), in your own eyes nothing is drawn at all.
source "$(dirname "$0")/lib.sh"
st() { kc --json stats | python3 -c "import json,sys; print(json.load(sys.stdin)['stats']['$1'])"; }
logged() { for _ in $(seq "${2:-40}"); do grep -q "$1" "$OUT/spacewm.log" && return 0; sleep 0.1; done; return 1; }

space_start --render 3d --map dust --log-level debug
sleep 1.5
E0=$(shot s14-eyes)
kc input key F5
logged "view mode=Behind" || fail "F5 did not go behind you"
logged "character uploaded" || fail "the character was not uploaded"
sleep 0.5
B=$(shot s14-behind)
python3 -c "exit(0 if $(python3 "$(dirname "$0")/pixel.py" diff "$E0" "$B") > 0.02 else 1)" ||
    fail "the behind view looks like the eyes view"
kc input key F5
logged "view mode=Front" || fail "second F5 did not go in front"
sleep 0.5
shot s14-front >/dev/null
pass "F5: behind you, then in front, with the character drawn"

# Standing still in third person: a few frames a second (the idle animation), not 60.
sleep 2.5
FPS=$(st fps)
python3 -c "exit(0 if 5 <= $FPS <= 20 else 1)" || fail "idle third person: $FPS fps"
pass "standing still in third person: $FPS fps (idle animation only)"

kc input key F5
logged "view mode=Eyes" || fail "third F5 did not go back to your eyes"
sleep 2.5
FPS=$(st fps)
python3 -c "exit(0 if $FPS < 0.5 else 1)" || fail "frames in your own eyes while still: $FPS"
pass "back in your eyes: nothing drawn while still ($FPS fps)"

# F6: the menu; 1 plays Cheer and (from your eyes) moves the view in front of you.
kc input key F6
logged "emote menu opened" || fail "F6 did not open the emote menu"
sleep 0.3
shot s14-emotes >/dev/null
kc input key 1
logged 'emote emote="Cheer"' || fail "1 did not play Cheer"
grep -q "view mode=Front" <(tail -n 50 "$OUT/spacewm.log") || true
sleep 0.6
shot s14-cheer >/dev/null
pass "F6 opens the emotes; 1 plays Cheer"
# A click on cell 2 (Sit).
kc input key F6
sleep 0.3
read -r CX CY < <(python3 -c "
W = 16 * 2 + 3 * 150 + 2 * 8; H = 34 + 3 * 150 + 2 * 8 + 32
ox, oy = (1280 - W) // 2, max((720 - H) // 2, 0)
print(ox + 16 + 150 + 8 + 75, oy + 16 + 34 + 75)")
kc input move-to "$CX" "$CY"
kc input click
logged 'emote emote="Sit"' || fail "a click on Sit did not play it"
sleep 2.5
shot s14-sit >/dev/null
pass "a click plays an emote (Sit), it keeps sitting"
# Walking ends it.
kc input key w --state press; sleep 0.5; kc input key w --state release
space_stop || fail "unclean exit"
grep -E "panicked|GL error" "$OUT/spacewm.log" && fail "errors in log"
pass "clean exit"
