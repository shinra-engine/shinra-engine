#!/bin/bash
# Build the wasm route: the engine, then a game's modules, then the glue.
#
# Two independent pipelines means two build paths. This is the wasm one; the
# `.so` one is `shinra build`.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ENGINE="$(cd "$HERE/.." && pwd)"
GAME="${1:-game1}"
# The engine may be a git worktree, so `--show-toplevel` is not the main
# checkout. Walk up until a sibling `shinra-examples` appears.
find_examples() {
  local d="$1"
  while [ "$d" != "/" ]; do
    [ -d "$d/shinra-examples" ] && { echo "$d/shinra-examples"; return; }
    d="$(dirname "$d")"
  done
  return 1
}
EXAMPLES="${SHINRA_EXAMPLES:-$(find_examples "$ENGINE")}"
[ -d "$EXAMPLES" ] || { echo "cannot find shinra-examples from $ENGINE" >&2; exit 1; }

cd "$ENGINE"
cargo build -q -p se --target wasm32-unknown-unknown
cargo build -q -p se-web --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir "$HERE/pkg" --no-typescript \
  target/wasm32-unknown-unknown/debug/se_web.wasm

# The generated module roots come from the `.so` build, which is also what
# writes `.gen/`. They are the same source either way.
cargo run -q --bin shinra -- build "$EXAMPLES/$GAME" >/dev/null

WDEP="$ENGINE/target/wasm32-unknown-unknown/debug/deps"
HDEP="$ENGINE/target/debug/deps"
SE="$(ls "$WDEP"/libse-*.rlib | head -1)"
OUT="$HERE/games/$GAME"
rm -rf "$OUT"; mkdir -p "$OUT"

# `data.rs` first: every other module compiles against it, which is what makes
# "obeys the bundle contract" a compile-time fact on this route too.
rustc --edition 2021 --target wasm32-unknown-unknown \
  -L "dependency=$WDEP" -L "dependency=$HDEP" --extern "se=$SE" \
  --crate-type rlib --crate-name data "$EXAMPLES/$GAME/bundle/data.rs" \
  -o "$OUT/libdata.rlib"

names=()
for root in "$ENGINE/target/shinra/$GAME/.gen/"*_root.rs; do
  b="$(basename "$root" _root.rs)"
  # `--export-table`: a control module's `start`/`tick`/`stop` are indices into
  # its *own* function table, so the host can only call them through that
  # table — and a table nobody exports is a table nobody can reach.
  rustc --edition 2021 --target wasm32-unknown-unknown \
    -L "dependency=$WDEP" -L "dependency=$HDEP" \
    --extern "se=$SE" --extern "data=$OUT/libdata.rlib" \
    -C link-arg=--export-table \
    --crate-type cdylib --crate-name "${b##*_}" "$root" -o "$OUT/$b.wasm"
  names+=("$b")
done
rm -f "$OUT/libdata.rlib"

printf '[%s]\n' "$(printf '"%s",' "${names[@]}" | sed 's/,$//')" > "$OUT/modules.json"
echo "$GAME: ${#names[@]} modules -> $OUT"
