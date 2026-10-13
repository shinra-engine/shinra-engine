# Shinra Engine

A game engine where every part of a game is a `.wasm` module that can be
replaced while the game is running.

The design is called **MCDV** — like MVC, split along the lines a game actually
changes along:

| | Module | Is | Swap it and |
|---|---|---|---|
| **M** | `asset.wasm` | model — meshes, textures, text | the game is about something else |
| **C** | `game.wasm` | control — input in, control vars out | the rules change |
| **D** | `data.wasm` | data pipeline — stages over columns | the world behaves differently |
| **V** | `render.wasm` | graphics pipeline — draw commands | the game looks different (2d, 3d, pixel art) |

All four obey one shared spec, `contract.wit`. **A module knows the spec and
nothing else.** It never knows which other modules are loaded or how they work —
a `render.wasm` does not know whether the data pipeline integrates physics or
replays a recording, only that `transform` is 48 bytes at a column the spec
names. So **any wasm that follows the spec can be hot-replaced**, by one written
by someone else, long after the engine was built. The world — every row, every
control var — survives every swap, because the host owns it and the modules
only read and write it.

Change `contract.wit` and you have a different game, so everything restarts.
That is the one line you cannot cross, and it is what makes the other four
cheap. The feel this is for is *gametok*: swipe, and you are in another game.

## The flow is a DAG

```
                        contract.wit
              the shared spec: every byte's place
                             │
        ┌────────────────────┼──────────────────────┐
        ▼                    ▼                      ▼
   M  asset.wasm        C  game.wasm  ◄── input  (keyboard, mouse)
      appearance           control
        │                    │ control vars
        │                    ▼
        │               D  data.wasm
        │                  data pipeline  ──►  columns (the world)
        │                    │
        └──────────┬─────────┘
                   ▼
              V  render.wasm  ──►  commands  ──►  host replays on the GPU  ──►  screen
                 graphics
```

One frame walks it in order, and nothing flows backwards:

1. The host writes `clock` (a fixed 1/60 s step) and `input`; a game never writes
   the clock. **C** `tick(frame)` reads them and writes the control var set —
   camera, how many rows each pool should have. Nothing else. The world is a
   function of the arena and the inputs, so two runs fed the same input are
   byte-identical.
2. The host clamps each pool's length to its capacity. That is the whole of
   spawn and despawn: control asks for a roster by writing one number.
3. **D** every stage runs over its pool's rows, reading control vars and
   columns, writing columns.
4. **V** `record()` writes a fixed-size command list; the host replays it with
   wgpu. **M** never runs: the host reads its bytes and publishes them as blobs
   that V draws and C may read.

None of the calls takes data or returns data the host acts on. Everything is
in the arena at addresses the contract fixed — which is why any module can be
replaced between any two frames.

## contract.wit — the shared spec

```wit
package shinra:game1@0.1.0;

interface components {          // per-row data; a stage writes one only via &mut
  use shinra:engine/types.{vec3, vec4};   // the engine's own package
  record transform { pos: vec3, rot: vec4, scale: vec3, kind: u32 }
  record orbit { center: vec3, radius: f32, speed: f32, phase: f32, tilt: f32 }
}

interface pools {               // columns that share one length: row i is one body
  use components.{transform, orbit};
  record bodies { t: transform, o: orbit }
}

interface control {             // the whole write surface of game.wasm
  record look  { asset: u32, graph: u32, bodies: u32 }
}
```

The engine's own records — `types`, `handles`, `clock`, `input`, `command`, `frame` —
are a second package, `shinra:engine`, injected by `se-contract`; a game reaches them
with `use shinra:engine/types.{…}`, never restates them, and one that tries is refused.
The whole contract hashes to one `CONTRACT_ID` in `contract.rs`.

WIT says *what* a field is. WGSL says *where* it sits: every layout is std430,
because the GPU reads these records too (`vec3` aligns to 16 but stays 12 bytes
— the reason layouts are generated, never written). `shinra build` turns the one
file into three, and nobody restates a layout:

| Generated | Read by |
|---|---|
| `contract.rs` | every module — `#[repr(C)]` structs padded to std430, `arena::*` addresses |
| `contract.wgsl` | every shader — prepended, so an instance input *is* the component |
| `contract.wasm` | the host, at run time — so one engine binary runs any game |

Field types are `f32`, `s32`, `u32`, `vec2/3/4`, and `string`/`list<T>`, which
lower to an 8-byte `span` into the blob heap.

## What makes hot replacement safe

All modules share **one linear memory the host owns**. Swapping a module cannot
disturb the world because the world is not in any module, and nothing is copied
to hand it over. That only holds under rules the engine enforces rather than
trusts:

- **Import nothing but `env::memory`.** Every module's stack is the same
  `[0, 64 KiB)`; sharing it is safe only while one module is ever on it, and a
  module that cannot call out cannot be suspended inside another. The loader
  refuses any other import, and any module exporting its own memory.
- **The arena starts at the stack top** (`0x10000`), so a stack overflow traps
  instead of silently eating the world.
- **Statics by kind, not load order.** `--global-base` puts C at `0x800000`, D at
  `0xC00000`, V and M at `0x1000000`, so a replacement lands exactly where its
  predecessor was. A swap is re-instantiation, not relocation.
- **Every module carries the contract's fingerprint.** Each embeds `CONTRACT_ID`
  in an `se.contract-id` section; one built against another
  contract (or by hand, with none) is refused before it touches memory.
- **Checked before it touches memory.** A module is compiled and checked —
  imports, its entry export, D's stages against the contract — before it is
  instantiated, so a refusal changes nothing.
- **The next frame confirms.** A swap is pending until a frame succeeds; a
  frame that traps re-instantiates the previous module from its bytes. Every
  call into wasm runs on a fuel budget, so an infinite loop is cut off, named,
  and rolled back. The world itself is not transactional.

## A game is a directory

There is no manifest; the tree is the configuration. **A `.rs` at depth 1 is a
module. A folder named after a sibling `.rs` is private to it; any other folder
is shared across the category.**

```
game1/
├── contract.wit                         the spec — required
├── asset/    bunny.rs  teapot.rs  bunny/model.obj         M
├── game/     game1.rs                                     C
├── process/  orbit.rs  spin.rs  breathe.rs  math/mod.rs   D
└── render/   solid.rs  ghost.rs  solid/shader.wgsl        V
```

`process/` is additive: every stage in it builds into one data module.
`asset/`, `game/` and `render/` are choices: one module per file, one loaded at
a time. Each module is one `rustc` invocation to `wasm32-unknown-unknown`; no
`Cargo.toml`, no build script. The contract is not imported, it is the crate:
`contract.rs` is included at the root, so a module reaches everything through
`use crate::*`.

Assets get their ids by sorted file name, so `bunny.rs` and `teapot.rs` become
`assets::BUNNY` and `assets::TEAPOT`, and a draw names one by id. Adding or
removing an asset renumbers the rest, so the game restarts.

## Writing one

```rust
// game/game1.rs — C: read input, write control vars
#[no_mangle]
pub extern "C" fn tick(frame: u32) -> u32 {
    unsafe {
        let input = &*(arena::INPUT as *const Input);
        let look = &mut *(arena::LOOK as *mut Look);
        if input.pressed & KEY_M != 0 { look.asset = (look.asset + 1) % 3; }
        look.bodies = 4;                                   // ask for a roster
        (*se_pool_mut(arena::POOL_BODIES)).len = look.bodies;
    }
    0
}
```

```rust
// process/orbit.rs — D: the parameter list is the query
#[se::stage]
fn orbit(o: &mut Orbit, t: &mut Transform) {
    let dt = unsafe { (*(arena::CLOCK as *const Clock)).dt };
    o.phase += o.speed * dt;
    t.pos = [o.center[0] + cos(o.phase) * o.radius, 0.0, o.center[2] + sin(o.phase) * o.radius];
}
```

Two rows at once — a hit, a pull — is `#[se::pair]`. Each parameter is one row of
one pool, as a tuple of components:

```rust
// process/hit.rs — D: every pair of rows; one pool visits i<j once, two pools i×j
#[se::pair]
fn hit((a, sa): (&mut Actor, &Sprite), (b, sb): (&mut Actor, &Sprite)) {
    if overlaps(sa, sb) { a.hp -= 1.0; b.hp -= 1.0; }
}
```

Stages run in name order, so a pipeline that needs one stage first names it so.

`#[se::stage]` writes an `se.stages` entry naming each component and whether it
is `&` or `&mut`, and exports the batched loop — the per-row loop runs inside
the module, one boundary crossing per stage. The host checks the entry against
the contract: unknown components, a component named twice, or columns from two
pools are refused.

```rust
// render/solid.rs — V: record commands; the host replays them
#[used] #[link_section = "se.wgsl"]
static WGSL: [u8; include_bytes!("solid/shader.wgsl").len()] = *include_bytes!("solid/shader.wgsl");

#[no_mangle]
pub extern "C" fn record() -> u32 {
    unsafe {
        let cmds = arena::COMMANDS as *mut Command;
        *cmds.add(0) = Command { kind: render::BEGIN_PASS, ..render::BLANK };
        *cmds.add(1) = Command { kind: render::DRAW_INSTANCED, vertices: 12,
            column: arena::COL_TRANSFORM, mesh: assets::BUNNY, texture: render::NONE, count: se_pool(arena::POOL_BODIES).len,
            uniform_off: arena::CAMERA, uniform_len: Camera::SIZE, ..render::BLANK };
        *cmds.add(2) = Command { kind: render::END_PASS, ..render::BLANK };
        let f = &mut *(arena::FRAME as *mut Frame);
        f.commands = arena::COMMANDS; f.count = 3;
        3
    }
}
```

```rust
// asset/bunny.rs — M: no code, just bytes
se::asset!("bunny/model.obj");
```

A draw carries a column *index*, not a pointer or a copy: the instance buffer
the host uploads is the column's own bytes at its own stride.

## Crates

| Crate | Role |
|---|---|
| `se-contract` | `contract.wit` → `contract.rs` + `contract.wgsl` + `contract.wasm`; std430 layout, arena map, `se.stages` format |
| `se-macro` | `#[se::stage]` |
| `se` | what a module compiles against: `no_std`, re-exports `#[se::stage]` |
| `se-vm` | the wasmtime host: one memory, the world (pools, columns, blobs), load/swap/rollback, fuel, saves, input |
| `se-gpu` | headless wgpu: replays the command list, reads back pixels |
| `se-tui` | the terminal IDE: panels, text-art viewport, log |
| `se-cli` | the `shinra` binary: discover, build, run, reload, shot |

The engine provides general tools. It has no camera, no sprite, no dialogue and
no physics — those are things games have, and games live in
[`shinra-examples`](../shinra-examples).

## Using it

```bash
rustup target add wasm32-unknown-unknown
cargo build
export SHINRA_ENGINE=$PWD

target/debug/shinra build ../shinra-examples/game1
target/debug/shinra run   ../shinra-examples          # a deck: every game in it
target/debug/shinra run   ../shinra-examples/game1    # one game
target/debug/shinra shot  ../shinra-examples/game1 -n 3 --size 160x90 > f.ppm
```

`run` opens the IDE: hierarchy (pools and rows), viewport, inspector (fields
decoded from the contract, editable), project tree and a log. While it runs it
polls the sources every 250 ms and hot-replaces whatever changed; a module
that fails to build or traps is reported in red and the previous one keeps
running. `--no-watch` turns that off.

| Key | |
|---|---|
| `r` | toggle Edit / Play |
| `n` / `p` | next / previous game in the deck (`p` in Edit) |
| `m` | viewport glyphs: mixed, quadrant, braille (Edit) |
| `tab`, `ctrl+h/v/i/f/t` | focus a panel |
| `F5` / `F9` | quick save / load — a save is the arena, byte for byte |
| `esc` | leave Play; in Edit, quit (`q` too) |

In Play every other key goes to the game through `control.input`.

### No GPU?

wgpu picks a software adapter. On Debian/Ubuntu:

```bash
apt-get install -y mesa-vulkan-drivers libvulkan1
```

This is a supported configuration, not a fallback — the engine is headless and
never opens a window. `docker compose run ide` uses an image with it installed;
`docker compose run ci` runs `scripts/ci.sh`.

## Known limits

- **One pass.** `command.shader` and `frame.present` are reserved: the replayer
  does not yet switch shaders or present a pass's output to another. Multi-pass V
  is the next piece of work.
- **Pair stages are brute force O(n·m).** A pair stage visits every pair inside
  the module; there is no broadphase.
- **The aspect ratio is a constant** in the shaders until the viewport is a
  control var.
- **Rollback restores modules, not the world.** Writes a trapping frame made
  before it trapped stay.

## Notes from building it

- **Uniform stack = arena base.** `--global-base` moves a module's statics but
  not its stack, and rustc always passes `--stack-first`, so every module's
  stack is `[0, stack-size)`. The contract's base must equal it; the host
  refuses a contract that disagrees.
- **A module loses its statics when it is swapped.** State that must survive is
  a component or a control var — which is the spec's job, not a module's.

## Licence

See [LICENSE](LICENSE).
