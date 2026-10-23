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
