# DRM tests on the real display

These take over the screen for a short time, so they are NOT part of `just e2e`, and
they need root: `systemd-run` (to start spacewm as a real logind session on tty8, like a
login manager would), `chvt`, and `/dev/uinput` (a virtual keyboard to press
Ctrl+Alt+F1, Mod+Enter and Ctrl+Alt+Backspace). spacewm itself still runs as your user.

Both scripts start an independent systemd timer that stops spacewm and returns to tty1
after 80-90 s, whatever happens to the script.

- `soak.sh`: run for `DURATION` seconds (default 60) with a foot window, screenshot twice
  a second into `~/spacewm-test/drm/`. `EXTRA="--render 2d"` passes more flags.
- `acceptance.py`: VT away and back (picture, apps and input return), our Ctrl+Alt+F1
  handling, keyboard input through libinput (Mod+Enter), emergency exit.

Paths assume the user `sar1k` (uid 1000) and the desktop on tty1; adjust at the top.
