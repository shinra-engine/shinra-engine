# The wasm route

Two independent pipelines. This is the wasm one; `shinra build` / `shinra run`
is the `.so` one. Nothing crosses.

```sh
./web/build.sh game1          # engine + glue + that game's modules
python3 -m http.server -d web # then open ?game=game1
```

`build.sh` compiles `se-web` for `wasm32-unknown-unknown`, runs `wasm-bindgen`
over it, and compiles each of the game's generated module roots to `.wasm`
against the same `data.rs` the `.so` build uses — so "obeys the bundle
contract" stays a compile-time fact here too.

`main.js` is the loader. There is no dlopen analogue in it: a module is
fetched and instantiated the way wasm modules are, and its exports are handed
to the engine. Replacing a module is instantiating another one and handing
over those exports instead.

## Verified

`web/routes_agree.sh` builds all three examples both ways and requires every
module to read identically. It is also a test — `crates/se-web/tests` — so it
runs with everything else.

```
all routes agree: 24 modules across 3 games
```

The same `bundle` source, built both ways, reads identically:

```
layout Transform size=40 align=4 hash=0x2995270e75f7499c
layout Orbit     size=28 align=4 hash=0x81172fd5632e3bf6
layout Spin      size=20 align=4 hash=0x4b1a0a11a37d3cbe
layout Breathe   size=16 align=4 hash=0xd4a4e348440914c8
layout Camera    size=40 align=4 hash=0xadb6f8051fdbf903
layout Look      size=12 align=4 hash=0xaad378119f872d76
buffer scene Rgba8Unorm   sampled=true
buffer depth Depth32Float sampled=false
buffer trail Rgba8Unorm   sampled=true
```

Every hash matches. The contract is the same contract.

## Rendering

`main.js` hands the engine the bundle, one render module, one asset module and
one game module — that is the slot, chosen here because instantiation is chosen
here and the control's own `set_slot` is recorded but not yet acted on. The
engine builds the `Targets` from `bundle/buffer.rs`, the `Graph` from
`render/*.rs`, and runs it. No WGSL lives in `se-web`; every shader is the one
the module carries.

The frame reaches the canvas one of two ways, decided from the bundle's own
buffer declaration before the canvas is given a context:

- **swapchain** — the presented buffer is screen-sized `Rgba8Unorm`, so it is
  copied straight into the swapchain image. All three examples take this path.
- **2d blit** — anything else is read back through `readback::read_async` and
  written into a 2D context.

`?game=`, `?graph=`, `?asset=`, `?w=`, `?h=` select what is drawn.

## One world

`openGame(names, exports, w, h)` answers with a handle that owns the world, the
loaded bundle and — from the first `render` — the device. `game.tick(n)` runs
the game's own control against that world; `game.render(canvas)` runs the graph
against the same one. That is `se-cli`'s `Session` on this route: the world is a
field of the handle, so control's writes are what the graph reads.

The two halves used to be `tickGame` and `renderGame`, two free functions each
standing up a `Box<dyn Store>` of its own — so the ticked world and the drawn
world were two worlds and a frame drew nothing. They are methods now because a
free function has nowhere to keep a world. `describeModule` and `planGame` are
still free functions; neither holds state.

The order within a frame is design.md's order — control decides, the stages
update the data, then the graph turns data into buffers — and `main.js` calls
them in it. All three stations, in the order `Session::tick` runs them.

## Control

A control module cannot be *handed* a `Ctl`: a function pointer is an index in
the calling module's own table and `ctx` is an offset in its own memory, so a
vtable the host filled in names nothing that module can reach. It imports the
host instead — `se_ctl_spawn`, `se_ctl_set`, … — the way `se`'s allocator
already does. `engineImports()` answers every one of them, and the world behind
them is `se-ecs` through `dyn Store`, the same one the native host drives.

Both directions of the call obey the same rule. Pointer arguments the module
passes are offsets into *its* memory, read and written live through its
`WebAssembly.Memory`; the `Ctl` and the `Frame` the host passes *in* are
written into that same memory with the module's own `se_alloc`. `tick` is a
table index inside the module, so it is called through the module's exported
table — which is why `build.sh` passes `-C link-arg=--export-table`.

`?control=` picks the game module, `?ticks=` how many ticks to run. Outside a
browser (no GPU, so `tick` only — a frame needs a canvas):

```sh
./web/build.sh game1
wasm-bindgen --target nodejs --out-dir .tmp/node --no-typescript \
  target/wasm32-unknown-unknown/debug/se_web.wasm
node -e "
const fs=require('fs'), se=require(process.cwd()+'/.tmp/node/se_web.js');
(async()=>{
  const names=['bundle','asset_bunny','game_game1'], ex=[];
  for (const n of names) ex.push((await WebAssembly.instantiate(
    await WebAssembly.compile(fs.readFileSync('web/games/game1/'+n+'.wasm')),
    se.engineImports())).exports);
  process.stdout.write(se.openGame(names, ex, 100, 56).tick(3));
})();"
```

```
control `orbit` in `game_game1`, 3 ticks
    binds Asset -> bunny
    binds Render -> solid
    log: game1: roster reconciled
    Transform x6 first: pos=0.000,0.000,0.000 rot=0.000,0.000,0.000,1.000 …
    Camera x1 first: eye=0.053,1.964,8.783 … yaw=0.006 pitch=0.220
    Look x1 first: asset=0 graph=0 bodies=6
```

Nothing in `se-web` wrote any of that. `start` reconciled the roster and the
three ticks drifted the camera by `0.12 * dt` each — `0.006` is exactly three
sixtieths of that — and the host read it back out of its own world. It is the
world a `render` then draws: six bodies rather than none.

### What the transport costs

`se_ctl_ptr` is the one call that cannot mean here what it means natively. It
answers with a pointer into the column, and the column is in the *host's*
memory, which the module cannot address. So the component is copied into the
module's memory and copied back when the tick returns. A game that borrowed a
component and then `get`s the same one later in the same tick would read the
old value; none of the three examples does. `se_ctl_asset` is the same copy,
kept for as long as the host is, because the wrapper promises `&'static`.

`se_ctl_asset_names` has no wrapper and no import on either route, so there is
nothing here for it to answer. Every other `se_ctl_*` is supplied and real.

## Stages

`se_register_stages` answers empty on wasm32: a custom section there must be a
plain run of bytes and a pointer to a `StageSpec` is a relocation, so
`#[se::stage]` exports each stage under its own name instead. `stage.rs`
enumerates those names — `se_stage__<name>` answers with the spec, and
`se_stage_run__<name>` is the batched loop — so a game that adds a stage adds a
file and nothing in the engine is edited. The spec is the same one the `.so`
route reads, built by the same attribute from the same signature, so the
parameter list *is* the query on both routes.

The columns cross the way `se_ctl_ptr`'s do. A stage runs in the module's
memory and the store lives in `se-web`'s, so each column the signature names is
copied in, the stage runs over the copy, and the columns declared `&mut` are
copied back. A `&T` parameter cannot have been written — the shim hands it out
as `*const` — so nothing else comes home.

Order is the native order, arrived at from the loader's other end: native takes
`process/*.so` in sorted path order, and this takes the modules in sorted name
order. Within one module the native order is the link section's and this is the
export section's; neither is declared anywhere, and every stage in the three
examples is alone in its module, so the two cannot yet disagree. A `process`
module carrying two stages is where that would have to be settled.

## Not there yet

Slots. A tick's `set_slot` is read back and reported, but nothing rebinds:
`main.js` still chooses which render module and which asset module go in.

A frame is still drawn once rather than in a loop, and `stop` is never called —
a game ends by the handle being dropped, and there is no unload hook to hang it
on.
