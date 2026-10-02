#!/usr/bin/env bash
# One entry point for CI: the same steps locally, in Docker and on a runner.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
export SHINRA_ENGINE="$PWD"

TARGET=wasm32-unknown-unknown

# --- engine ---------------------------------------------------------------
# se-vm / se-gpu tests and every module build need the wasm32 target.
has_target() { [ -d "$(rustc --print target-libdir --target "$TARGET" 2>/dev/null)" ]; }

if ! has_target; then
    if command -v rustup >/dev/null 2>&1; then
        echo "ci: installing $TARGET" >&2
        rustup target add "$TARGET"
    fi
    has_target || {
        echo "ci: error: the $TARGET target is missing; run: rustup target add $TARGET" >&2
        exit 1
    }
fi

cargo build -p se-cli
# Explicit list, not --workspace: legacy crates (se-host, se-render, most of
# se-abi) must not be able to fail CI.
cargo test -p se-contract -p se-macro -p se -p se-vm -p se-gpu -p se-cli -p se-tui

echo "ci: engine ok"
