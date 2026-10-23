#!/bin/bash
# Run spacewm on the real display (tty5) for 60 s, screenshot twice a second, then return to tty1.
set -u
SOCK=/run/user/1000/spacewm-drmtest.sock
OUT=${OUT:-/home/sar1k/spacewm-test/drm}
DURATION=${DURATION:-60}
KC="runuser -u sar1k -- env XDG_RUNTIME_DIR=/run/user/1000 /home/sar1k/spacewm/target/debug/spacectl --socket $SOCK"
rm -rf "$OUT"; runuser -u sar1k -- mkdir -p "$OUT"
rm -f $SOCK
systemctl reset-failed spacewm-drmtest.service spacewm-drmtest-guard.service 2>/dev/null

# Independent guard: whatever happens to this script, give the display back after 80 s.
systemd-run --quiet --unit=spacewm-drmtest-guard --on-active=80 \
  /bin/sh -c 'systemctl stop spacewm-drmtest.service; sleep 1; chvt 1'

chvt 8; sleep 0.5
systemd-run --quiet --unit=spacewm-drmtest \
  -p PAMName=login -p User=sar1k -p TTYPath=/dev/tty8 -p StandardInput=tty \
  -p StandardOutput=journal -p StandardError=journal -p TTYReset=yes -p TTYVHangup=yes \
  -p WorkingDirectory=/home/sar1k \
  /home/sar1k/spacewm/target/debug/spacewm --backend drm --no-session-env \
    --ipc-socket $SOCK --layout $OUT/layout.json --spawn foot ${EXTRA:-}

start=$(date +%s%N)
for i in $(seq 1 40); do [ -S $SOCK ] && break; sleep 0.25; done
[ -S $SOCK ] || echo "IPC socket never appeared"
n=0
while :; do
  el=$(( ($(date +%s%N) - start) / 1000000 ))ms
  (( ${el%ms} >= DURATION * 1000 )) && break
  systemctl is-active --quiet spacewm-drmtest.service || { echo "spacewm exited at ${el}"; break; }
  n=$((n+1)); f=$(printf "%s/shot-%03d.png" $OUT $n)
  timeout 2 $KC screenshot "$f" >/dev/null 2>&1 || echo "shot $n failed at ${el}"
  sleep 0.5
done
timeout 3 $KC quit >/dev/null 2>&1
sleep 2
systemctl stop spacewm-drmtest.service 2>/dev/null
sleep 1
chvt 1
systemctl stop spacewm-drmtest-guard.timer 2>/dev/null
echo "shots taken: $(ls $OUT/*.png 2>/dev/null | wc -l)"
