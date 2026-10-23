#!/usr/bin/env bash
# GPU (dmabuf) clients and GTK popups.
source "$(dirname "$0")/lib.sh"
space_start --render 2d
kc spawn weston-simple-egl >/dev/null
wait_windows 1
sleep 1
shot s1-egl >/dev/null
grep -q "rejecting dmabuf" "$OUT/spacewm.log" && fail "dmabuf import rejected"
pass "EGL client (dmabuf) renders"
kc close 1; sleep 0.5

kc spawn gtk3-demo >/dev/null
wait_windows 1
sleep 2
# Right-click the demo list to open a context menu? gtk3-demo has a menubar-less UI; open
# the "Help/About" via F1 is unreliable, so use the source view's context menu.
kc input move-to 700 300
kc input click right
sleep 1
shot s1-gtk-popup >/dev/null
pass "gtk3-demo with popup screenshot taken"
kc input key Escape
space_stop || fail "unclean exit"
grep -E "panicked|ERROR" "$OUT/spacewm.log" && fail "errors in log"
pass "no errors"
