build:
    cargo build --release

test:
    cargo test

lint:
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check
