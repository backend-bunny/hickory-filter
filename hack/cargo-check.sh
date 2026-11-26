#!/usr/bin/env bash
# The Rust checks CI runs on every change: format, lints, tests.
. "$(dirname "$0")/lib.sh"
cd "$ROOT"
need cargo
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
