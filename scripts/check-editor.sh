#!/usr/bin/env bash
# Routine checks; the optional native-worker acceptance suite remains separate.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
python3 scripts/check-editor-core-boundary.py
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked -p a-less-awful-editor
