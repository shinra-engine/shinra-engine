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

# --- examples -------------------------------------------------------------
EXAMPLES=${EXAMPLES:-../shinra-examples}
W=160 H=90
HEADER=$(printf 'P6\n%d %d\n255\n' "$W" "$H")
HLEN=${#HEADER}
HLEN=$((HLEN + 1)) # $(...) strips the trailing newline

# A PPM of the right header and size, whose pixels are not all one colour.
validate_ppm() {
    local f=$1 size want colours
    if [ "$(head -c "$HLEN" "$f" | od -An -c -v | tr -d ' \n')" != \
         "$(printf 'P6\n%d %d\n255\n' "$W" "$H" | od -An -c -v | tr -d ' \n')" ]; then
        echo "ci: error: $f: bad PPM header" >&2
        return 1
    fi
    size=$(wc -c < "$f")
    want=$((HLEN + W * H * 3))
    if [ "$size" -ne "$want" ]; then
        echo "ci: error: $f: $size bytes, expected $want" >&2
        return 1
    fi
    colours=$(tail -c +$((HLEN + 1)) "$f" | od -An -tu1 -v -w3 | sort -u | wc -l)
    if [ "$colours" -le 1 ]; then
        echo "ci: error: $f: uniform frame ($colours colour)" >&2
        return 1
    fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for dir in "$EXAMPLES"/game*/; do
    dir=${dir%/}
    g=$(basename "$dir")
    if [ ! -f "$dir/contract.wit" ]; then
        echo "ci: skip $g (no contract.wit)" >&2
        continue
    fi
    echo "ci: example $g" >&2
    target/debug/shinra build "$dir"
    target/debug/shinra shot "$dir" -n 3 --size "${W}x${H}" > "$tmp/$g.ppm"
    validate_ppm "$tmp/$g.ppm"
done

echo CI OK
