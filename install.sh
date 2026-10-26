#!/usr/bin/env bash
# Install spacewm on an Arch-based system (Arch, CachyOS, ...): the packages it needs,
# the build, the login session and SDDM as the login manager.
#
# Run it as your normal user, NOT as root. It calls sudo itself, only for the steps that
# write to the system (pacman, /usr/local/bin, /usr/share, systemctl).
# GRUB is not touched: spacewm needs nothing from the bootloader.
#
#   ./install.sh                    install everything
#   ./install.sh --dry-run          print what would run, change nothing
#   ./install.sh --no-test-tools    only spacewm and what it needs (no foot, weston, waybar...)
#
# Wayland/Smithay notes for later reading:
# - "DRM" is how spacewm draws straight to the monitor without another compositor. It needs
#   a GPU with KMS; in a VM that means virtio-gpu (3D acceleration) turned on.
# - "libseat" lets a program that is not root open the GPU and input devices, through
#   systemd-logind here. So seatd is installed only as a library: its service stays off.

set -euo pipefail

REPO="$(cd "$(dirname "$0")" && pwd)"
DRY_RUN=0
TEST_TOOLS=1

# Packages spacewm needs at build time (pkg-config files) and at run time.
#   rust, base-devel, git, pkgconf: compile spacewm (git fetches smithay from GitHub)
#   seatd (libseat), systemd (libudev), libinput, mesa (gbm), libxkbcommon, wayland,
#   libdrm: system libraries the Rust crates link against
#   xorg-xwayland: X11 apps (Steam, games...) inside spacewm
#   xdg-desktop-portal-gtk / -wlr: file pickers and screenshots, see spacewm-portals.conf
#   sddm: the login screen
CORE_PACKAGES=(
    rust base-devel git pkgconf
    seatd systemd libinput mesa libxkbcommon wayland libdrm
    xorg-xwayland
    xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-wlr
    sddm
)

# Terminal, bar, notifications, launcher: used by spacewm's dock and the e2e tests.
TEST_PACKAGES=(foot weston waybar mako fuzzel libnotify xterm)

log() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Print the command in dry-run mode, run it otherwise.
run() {
    if [ "$DRY_RUN" = 1 ]; then
        printf '    would run: %s\n' "$*"
    else
        "$@"
    fi
}

usage() {
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
}

parse_args() {
    for arg in "$@"; do
        case "$arg" in
            --dry-run) DRY_RUN=1 ;;
            --no-test-tools) TEST_TOOLS=0 ;;
            -h|--help) usage; exit 0 ;;
            *) die "unknown option: $arg (try --help)" ;;
        esac
    done
}

check_system() {
    # Never as root: makepkg-style tools refuse it, and cargo would leave root-owned files
    # in the repo.
    [ "$(id -u)" -ne 0 ] || die "run install.sh as your normal user, not with sudo"
    [ -f /etc/arch-release ] || die "this installer is for Arch-based systems (pacman)"
    command -v pacman >/dev/null || die "pacman not found"
    command -v sudo >/dev/null || die "sudo not found: install it and add your user to the wheel group"
    [ -f "$REPO/Cargo.toml" ] || die "Cargo.toml not found next to install.sh ($REPO)"
}

check_other_login_managers() {
    # Two login managers would fight over the screen at boot. Stop before changing anything.
    local dm
    for dm in gdm lightdm greetd ly; do
        if systemctl is-enabled --quiet "$dm.service" 2>/dev/null; then
            die "$dm is enabled as the login manager. Disable it first: sudo systemctl disable $dm.service"
        fi
    done
}

install_packages() {
    local packages=("${CORE_PACKAGES[@]}")
    [ "$TEST_TOOLS" = 1 ] && packages+=("${TEST_PACKAGES[@]}")
    log "Installing packages with pacman (full system upgrade first, as Arch requires)"
    # --needed skips what is already there. A partial upgrade (-Sy without -u) is not
    # supported on Arch, so the sync is always a full -Syu.
    run sudo pacman -Syu --needed --noconfirm "${packages[@]}"
}

build() {
    log "Building spacewm (release). Cargo fetches smithay from GitHub on the first run."
    run cargo build --release --bins --manifest-path "$REPO/Cargo.toml"
}

install_files() {
    log "Installing spacewm into the system (sudo)"
    local bin="$REPO/target/release"
    run sudo install -Dm755 "$bin/spacewm" /usr/local/bin/spacewm
    run sudo install -Dm755 "$bin/spacectl" /usr/local/bin/spacectl
    run sudo install -Dm755 "$REPO/resources/spacewm-session" /usr/local/bin/spacewm-session
    run sudo install -Dm644 "$REPO/resources/spacewm.desktop" /usr/share/wayland-sessions/spacewm.desktop
    run sudo install -Dm644 "$REPO/resources/spacewm-portals.conf" /usr/share/xdg-desktop-portal/spacewm-portals.conf
}

enable_sddm() {
    # enable, not start: starting it now would take over the screen. The login screen
    # appears on the next boot. Autologin stays off: you pick "spacewm" in the menu.
    log "Enabling SDDM as the login manager (starts at next boot)"
    run sudo systemctl enable sddm.service
}

verify() {
    log "Checking the install"
    if [ "$DRY_RUN" = 1 ]; then
        echo "    (dry run: nothing to check)"
        return
    fi
    command -v spacewm >/dev/null || die "spacewm is not on PATH after install"
    command -v spacectl >/dev/null || die "spacectl is not on PATH after install"
    [ -f /usr/share/wayland-sessions/spacewm.desktop ] || die "session entry missing"
    # Reads ~/.config/spacewm/config.toml and reports errors; it creates nothing.
    "$REPO/target/release/spacewm" --check-config
    echo "    spacewm is installed. Reboot, then pick \"spacewm\" in the SDDM menu."
}

main() {
    parse_args "$@"
    check_system
    check_other_login_managers
    install_packages
    build
    install_files
    enable_sddm
    verify
}

main "$@"
