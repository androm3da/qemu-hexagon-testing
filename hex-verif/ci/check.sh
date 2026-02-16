#!/usr/bin/env bash

set -euo pipefail

export RUSTFLAGS="-D warnings"
# Check code formatting
cargo fmt --all -- --check

# Run Clippy with strict checks
cargo clippy --all-targets -- -D warnings

# Run the actual tests
cargo test --all-targets
cargo test --doc
