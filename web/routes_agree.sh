#!/bin/bash
# Build every example both ways and require the two routes to read the same
# contract out of each module. Called by crates/se-web/tests/routes_agree.rs.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ENGINE="$(cd "$HERE/.." && pwd)"
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
GLUE="${TMPDIR:-/tmp}/se_routes_node"

cd "$ENGINE"
cargo build -q --bin describe
mismatch=0; checked=0
for g in "$@"; do
  SHINRA_EXAMPLES="$EXAMPLES" "$HERE/build.sh" "$g" >/dev/null
  wasm-bindgen --target nodejs --out-dir "$GLUE" --no-typescript \
    target/wasm32-unknown-unknown/debug/se_web.wasm >/dev/null 2>&1

  for m in $(node -e "console.log(JSON.parse(require('fs').readFileSync('$HERE/games/$g/modules.json','utf8')).join(' '))"); do
    case "$m" in
      bundle) so="target/shinra/$g/bundle.so" ;;
      *)      so="target/shinra/$g/${m%%_*}/${m#*_}.so" ;;
    esac
    ./target/debug/describe "$so" > /tmp/routes_a.txt
    # A control module imports the host's se_ctl_* functions, so instantiating
    # one means answering them. Reading registrations never calls them; the
    # stubs are there so instantiation succeeds.
    node -e "
      const fs=require('fs'), se=require('$GLUE/se_web.js');
      (async()=>{
        const mod = await WebAssembly.compile(fs.readFileSync('$HERE/games/$g/$m.wasm'));
        const imports = {};
        for (const {module: m, name, kind} of WebAssembly.Module.imports(mod)) {
          if (kind !== 'function') continue;
          (imports[m] ??= {})[name] = () => { throw new Error(name + ': not wired here'); };
        }
        const instance = await WebAssembly.instantiate(mod, imports);
        process.stdout.write(se.describeModule('$m', instance.exports));
      })();" > /tmp/routes_b.txt
    grep -v 'registered nothing' /tmp/routes_b.txt > /tmp/routes_b2.txt || true
    if ! diff -q /tmp/routes_a.txt /tmp/routes_b2.txt >/dev/null; then
      echo "MISMATCH $g/$m"; diff /tmp/routes_a.txt /tmp/routes_b2.txt | head -8
      mismatch=$((mismatch+1))
    fi
    checked=$((checked+1))
  done
done
[ "$mismatch" -eq 0 ] || { echo "$mismatch of $checked modules disagree"; exit 1; }
echo "all routes agree: $checked modules across $# games"
