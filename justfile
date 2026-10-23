run *ARGS:
    cargo run -- {{ARGS}}

run-release *ARGS:
    cargo run --release -- {{ARGS}}

check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
