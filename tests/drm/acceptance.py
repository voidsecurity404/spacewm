#!/usr/bin/env python3
"""Stage 6 acceptance run on the real display (tty8), driven by a uinput keyboard."""
import ctypes, fcntl, os, struct, subprocess, sys, time

SOCK = "/run/user/1000/spacewm-drmtest.sock"
OUT = "/home/sar1k/spacewm-test/drm-acceptance"
SPACECTL = "/home/sar1k/spacewm/target/debug/spacectl"
log = []

def say(msg):
    line = f"[{time.monotonic()-T0:6.2f}] {msg}"
    print(line, flush=True)

def sh(*a, check=False):
    return subprocess.run(a, capture_output=True, text=True, check=check)

def kc(*args, timeout=3):
    try:
        r = subprocess.run(["runuser", "-u", "sar1k", "--", "env", "XDG_RUNTIME_DIR=/run/user/1000",
                            SPACECTL, "--socket", SOCK, *args], capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout + r.stderr).strip()
    except subprocess.TimeoutExpired:
        return -1, "timeout"

def vt():
    return open("/sys/class/tty/tty0/active").read().strip()

def alive():
    return sh("systemctl", "is-active", "--quiet", "spacewm-drmtest.service").returncode == 0

shot_n = 0
def shot(tag):
    global shot_n
    shot_n += 1
    rc, out = kc("screenshot", f"{OUT}/{shot_n:02d}-{tag}.png")
    say(f"screenshot {tag}: rc={rc} {out[:80]}")

# ---- uinput keyboard
UI_SET_EVBIT, UI_SET_KEYBIT, UI_DEV_SETUP = 0x40045564, 0x40045565, 0x405C5503
UI_DEV_CREATE, UI_DEV_DESTROY = 0x5501, 0x5502
EV_SYN, EV_KEY = 0, 1
KEYS = dict(CTRL=29, ALT=56, F1=59, BACKSPACE=14, META=125, ENTER=28)
fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
for code in KEYS.values():
    fcntl.ioctl(fd, UI_SET_KEYBIT, code)
setup = struct.pack("HHHH80sI", 0x06, 0x1234, 0x5678, 1, b"spacewm-test-keyboard", 0)
fcntl.ioctl(fd, UI_DEV_SETUP, setup)
fcntl.ioctl(fd, UI_DEV_CREATE)

def ev(t, c, v):
    os.write(fd, struct.pack("llHHi", 0, 0, t, c, v))

def combo(*names):
    for n in names:
        ev(EV_KEY, KEYS[n], 1); ev(EV_SYN, 0, 0); time.sleep(0.03)
    for n in reversed(names):
        ev(EV_KEY, KEYS[n], 0); ev(EV_SYN, 0, 0); time.sleep(0.03)
    say(f"pressed {'+'.join(names)}")

T0 = time.monotonic()
results = {}
try:
    sh("rm", "-rf", OUT); sh("runuser", "-u", "sar1k", "--", "mkdir", "-p", OUT)
    if os.path.exists(SOCK):
        os.unlink(SOCK)
    sh("systemctl", "reset-failed", "spacewm-drmtest.service", "spacewm-drmtest-guard.service")
    sh("systemd-run", "--quiet", "--unit=spacewm-drmtest-guard", "--on-active=90",
       "/bin/sh", "-c", "systemctl stop spacewm-drmtest.service; sleep 1; chvt 1")
    sh("chvt", "8"); time.sleep(0.5)
    r = sh("systemd-run", "--quiet", "--unit=spacewm-drmtest",
           "-p", "PAMName=login", "-p", "User=sar1k", "-p", "TTYPath=/dev/tty8",
           "-p", "StandardInput=tty", "-p", "StandardOutput=journal", "-p", "StandardError=journal",
           "-p", "WorkingDirectory=/home/sar1k",
           "-p", "Environment=SPACEWM_BIN=/home/sar1k/spacewm/target/debug/spacewm",
           "/home/sar1k/spacewm/resources/spacewm-session", "--no-session-env",
           "--ipc-socket", SOCK, "--layout", f"{OUT}/layout.json", "--config", f"{OUT}/config.toml", "--spawn", "foot")
    say(f"started: {r.returncode} {r.stderr.strip()}")
    for _ in range(60):
        if os.path.exists(SOCK): break
        time.sleep(0.25)
    if r.returncode != 0 or not os.path.exists(SOCK):
        raise SystemExit("spacewm did not start; aborting")
    time.sleep(3)  # foot maps, input devices appear
    shot("start")
    rc, panels = kc("list"); say("panels:\n" + panels)
    results["start"] = alive() and "foot" in panels

    # 1. VT away and back (like Ctrl+Alt+F2 from another program).
    sh("chvt", "1"); time.sleep(3); say(f"vt after chvt 1: {vt()}, spacewm alive={alive()}")
    sh("chvt", "8"); time.sleep(2); say(f"vt after chvt 8: {vt()}")
    shot("after-vt-return")
    rc, panels2 = kc("list")
    results["vt_return"] = alive() and "foot" in panels2 and vt() == "tty8"

    # 2. Our own Ctrl+Alt+F1 handling (libinput -> spacewm -> session.change_vt).
    was_alive = alive()
    combo("CTRL", "ALT", "F1"); time.sleep(2)
    results["ctrl_alt_f1"] = was_alive and alive() and vt() == "tty1"
    say(f"vt after Ctrl+Alt+F1: {vt()}")
    sh("chvt", "8"); time.sleep(2)

    # 3. Real keyboard input: Mod+Enter opens a terminal.
    combo("META", "ENTER"); time.sleep(2.5)
    shot("after-mod-enter")
    rc, panels3 = kc("list"); say("panels:\n" + panels3)
    results["mod_enter"] = panels3.count("foot") >= 2

    # 4. Emergency exit.
    combo("CTRL", "ALT", "BACKSPACE"); time.sleep(2)
    results["emergency_exit"] = results.get("mod_enter", False) and not alive()
    say(f"after Ctrl+Alt+Backspace: spacewm alive={alive()}")
finally:
    if alive():
        kc("quit"); time.sleep(2)
    sh("systemctl", "stop", "spacewm-drmtest.service")
    time.sleep(1)
    sh("chvt", "1")
    sh("systemctl", "stop", "spacewm-drmtest-guard.timer")
    fcntl.ioctl(fd, UI_DEV_DESTROY); os.close(fd)
    say(f"final vt: {vt()}")
    for k, v in results.items():
        print(f"  {'PASS' if v else 'FAIL'} {k}")
