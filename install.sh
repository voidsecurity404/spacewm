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
# Inside a VirtualBox VM the installer also: installs the VirtualBox guest tools, makes
# Alt the Mod key (VirtualBox passes Super to the host), and turns off direct panels and
# mipmaps in a NEW config file (an existing one is never changed). See README, "Running it
# in a virtual machine".
#
# Wayland/Smithay notes for later reading:
# - "DRM" is how spacewm draws straight to the monitor without another compositor. It needs
#   a GPU with KMS; in a VM that means 3D acceleration and the VMSVGA graphics controller.
# - "libseat" lets a program that is not root open the GPU and input devices, through
#   systemd-logind here. So seatd is installed only as a library: its service stays off.

set -euo pipefail

REPO="$(cd "$(dirname "$0")" && pwd)"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/spacewm"
CONFIG_FILE="$CONFIG_DIR/config.toml"
DRY_RUN=0
TEST_TOOLS=1
VIRT=none

# Packages spacewm needs at build time (pkg-config files) and at run time.
#   rust, base-devel, git, pkgconf: compile spacewm (git fetches smithay from GitHub)
#   seatd (libseat), systemd (libudev), libinput, mesa (gbm), libxkbcommon, wayland,
#   libdrm: system libraries the Rust crates link against
#   xorg-xwayland: X11 apps (Steam, games...) inside spacewm
#   xdg-desktop-portal-gtk / -wlr: file pickers and screenshots, see spacewm-portals.conf
#   sddm: the login screen
#   grim, slurp: screenshots (Print, Shift+Print)
#   swaylock: the lock screen (Mod+L, and after 10 min idle)
#   wireplumber (wpctl), pipewire: volume and mic keys
#   brightnessctl: brightness keys
#   mesa-utils: glxinfo, to check which GPU driver is in use
CORE_PACKAGES=(
    rust base-devel git pkgconf
    seatd systemd libinput mesa libxkbcommon wayland libdrm
    xorg-xwayland
    xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-wlr
    sddm
    grim slurp swaylock
    wireplumber pipewire brightnessctl
    mesa-utils
)

# Terminal, bar, notifications, launcher: used by spacewm's dock and the e2e tests.
TEST_PACKAGES=(foot weston waybar mako fuzzel libnotify xterm)

log() { printf '\n==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
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
    sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
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

detect_vm() {
    # systemd-detect-virt prints "oracle" for VirtualBox, "none" on real hardware.
    VIRT=$(systemd-detect-virt --vm 2>/dev/null || true)
    VIRT=${VIRT:-none}
    [ "$VIRT" = none ] || log "Running inside a virtual machine: $VIRT"
}

# The kernel headers that match the running kernel: DKMS builds the VirtualBox guest
# modules against them.
kernel_headers_package() {
    case "$(uname -r)" in
        *cachyos*) echo linux-cachyos-headers ;;
        *omarchy*) echo linux-omarchy-headers ;;
        *zen*) echo linux-zen-headers ;;
        *lts*) echo linux-lts-headers ;;
        *) echo linux-headers ;;
    esac
}

install_packages() {
    local packages=("${CORE_PACKAGES[@]}")
    [ "$TEST_TOOLS" = 1 ] && packages+=("${TEST_PACKAGES[@]}")
    if [ "$VIRT" = oracle ]; then
        # Guest tools: mouse integration, screen resize and the clipboard. The DKMS module
        # needs the headers of the running kernel.
        packages+=(virtualbox-guest-utils virtualbox-guest-dkms "$(kernel_headers_package)")
    fi
    log "Installing packages with pacman (full system upgrade first, as Arch requires)"
    # --needed skips what is already there. A partial upgrade (-Sy without -u) is not
    # supported on Arch, so the sync is always a full -Syu.
    run sudo pacman -Syu --needed --noconfirm "${packages[@]}"
    if [ "$VIRT" = oracle ]; then
        run sudo systemctl enable vboxservice.service
    fi
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

configure_vm() {
    # Only for VirtualBox, and only when there is no config yet: a config you already have
    # is never changed. The template keeps every option and its comment.
    [ "$VIRT" = oracle ] || return 0
    log "VirtualBox: writing a safe config for the VM"
    if [ -e "$CONFIG_FILE" ]; then
        warn "$CONFIG_FILE already exists and was left as it is. In a VM set there:"
        warn "  mod_key = \"alt\", direct_panels = false, panel_mipmaps = false"
        return 0
    fi
    if [ "$DRY_RUN" = 1 ]; then
        printf '    would write %s from resources/config.toml with mod_key = "alt",\n' "$CONFIG_FILE"
        printf '    direct_panels = false, panel_mipmaps = false\n'
        return 0
    fi
    mkdir -p "$CONFIG_DIR"
    {
        printf '# VirtualBox VM: written by install.sh. Super belongs to the host here, so Alt is\n'
        printf '# the Mod key; direct panels and mipmaps are off while the VM graphics are tested.\n'
        printf '# Set them back to true when the picture is stable.\n'
        sed -e 's/^mod_key = "super"/mod_key = "alt"/' \
            -e 's/^direct_panels = true/direct_panels = false/' \
            -e 's/^panel_mipmaps = true/panel_mipmaps = false/' \
            "$REPO/resources/config.toml"
    } >"$CONFIG_FILE"
    grep -q '^mod_key = "alt"' "$CONFIG_FILE" || die "could not set mod_key in $CONFIG_FILE"
    grep -q '^direct_panels = false' "$CONFIG_FILE" || die "could not set direct_panels in $CONFIG_FILE"
    grep -q '^panel_mipmaps = false' "$CONFIG_FILE" || die "could not set panel_mipmaps in $CONFIG_FILE"
    echo "    wrote $CONFIG_FILE"
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
    if [ "$VIRT" = oracle ]; then
        cat <<'EOF'
    VirtualBox checklist (do these in the VM settings, with the VM powered off):
      - Settings > Display: Graphics Controller = VMSVGA, Video Memory = 128 MB,
        Enable 3D Acceleration = on.
      - Settings > Input: the host key is Right Ctrl. Super is taken by the host, so
        spacewm uses Alt as Mod in this VM.
    After the reboot, if the picture is wrong: Ctrl+Alt+F3 for a terminal, then
      tail -60 ~/.local/state/spacewm/spacewm.log
    Ctrl+Alt+Backspace always leaves spacewm.
EOF
    fi
}

main() {
    parse_args "$@"
    check_system
    check_other_login_managers
    detect_vm
    install_packages
    build
    install_files
    configure_vm
    enable_sddm
    verify
}

# Run main only when executed, not when this file is sourced for testing.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    main "$@"
fi
