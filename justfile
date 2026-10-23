run *ARGS:
    cargo run -- {{ARGS}}

run-release *ARGS:
    cargo run --release -- {{ARGS}}

check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

# End-to-end tests: headless spacewm driven through spacectl (needs a GPU render node).
e2e:
    cargo build
    for t in tests/e2e/stage*.sh tests/e2e/winit_nested.sh; do echo "== $t"; "$t" || exit 1; done

# Print (never run) the sudo commands that install spacewm as a login session.
install:
    cargo build --release
    @echo "Run these yourself:"
    @echo "  sudo install -Dm755 target/release/spacewm  /usr/local/bin/spacewm"
    @echo "  sudo install -Dm755 target/release/spacectl /usr/local/bin/spacectl"
    @echo "  sudo install -Dm755 resources/spacewm-session /usr/local/bin/spacewm-session"
    @echo "  sudo install -Dm644 resources/spacewm.desktop /usr/share/wayland-sessions/spacewm.desktop"
    @echo "  sudo install -Dm644 resources/spacewm-portals.conf /usr/share/xdg-desktop-portal/spacewm-portals.conf"

# Show what `install` put in place and how to remove it.
uninstall:
    @echo "  sudo rm /usr/local/bin/spacewm /usr/local/bin/spacectl /usr/local/bin/spacewm-session"
    @echo "  sudo rm /usr/share/wayland-sessions/spacewm.desktop /usr/share/xdg-desktop-portal/spacewm-portals.conf"
