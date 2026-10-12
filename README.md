# Shinra Engine

A game engine where every part of a game is a `.so` that can be replaced while
the game is running.

The specification is [`design.md`](design.md). This file describes what is
built.

```
             bundle.so
         data.rs + buffer.rs
        ↙        ↓       ↘
   data ──►  game.so ──┬──►  process.so ────►  data
  status     control   │         ecs          update
                ↓      │
            asset.so ──┴──►  render.so ───►  buffer ──►  show
            appearance         graphs        targets

  └──── imperative ────┘└───── pure ───────┘
```

Change `render.so` and the game looks different. Change `asset.so` and it is
about something else. Change `game.so` and the rules change. The world — every
entity, every component — survives all three, because the host owns it and the
modules only describe it.

Change `bundle.so` and you have a different game set, so everything reloads.
That is the one line you cannot cross without starting over, which is what
makes the other three cheap.

## A game is a directory

There is no manifest. The tree is the configuration, because a manifest could
disagree with the tree.

**A `.rs` at depth 1 is a module. Anything deeper is a source file inside one.**

```
game1/
├── bundle/  data.rs  buffer.rs          both required
├── process/ orbit.rs  spin.rs  breathe.rs
├── render/  solid.rs  ghost.rs  solid/shade.rs  wgsl/mod.rs
├── asset/   bunny.rs  teapot.rs  bunny/model.obj
└── game/    game1.rs  showcase.rs
```

A folder named after a sibling `.rs` is private to it — `render/solid/` belongs
to `solid.rs` alone. One without a sibling is shared across the category —
`render/wgsl/`, reached as `crate::render::wgsl`.

Each module is one `rustc` invocation. No `Cargo.toml`, no build script.

## Six entry points

| Module | Symbol | Registers |
|---|---|---|
| `bundle.so` | `se_register_layouts()` | name → layout (from `data.rs`) |
| | `se_register_buffers()` | name → shape, count, sampled (from `buffer.rs`) |
| `asset/*.so` | `se_register_assets()` | name → bytes |
| `process/*.so` | `se_register_stages()` | spec + function |
| `render/*.so` | `se_register_graph()` | nodes + edges |
| `game/*.so` | `se_register_control()` | slots + tick |

Every module also exports `se_abi_version`, checked before anything else in
the image is trusted.

## Writing one

```rust
// bundle/data.rs
#[repr(C)]
#[derive(Clone, Copy, se::Schema)]
pub struct Transform { pub pos: [f32; 3], pub rot: [f32; 4], pub scale: [f32; 3] }

se::layouts!(Transform);
```

```rust
// process/orbit.rs — the parameter list IS the query
use data::{Orbit, Transform};

#[se::stage]
fn orbit(t: &mut Transform, o: &mut Orbit, dt: f32) { /* ... */ }

se::stages!(orbit);
```

Reaching outside the signature is not expressible. There is no world handle, no
command buffer, no escape hatch — so "who wrote this component?" is answered by
reading one file.

```rust
// render/solid.rs — pure: data + assets in, buffers out
se::graph!("solid", |g| g.present("scene").pass("bodies", |p| p
    .shader(format!("{}{}", wgsl::MESH_VS, shade::FS))
    .color(&["scene"]).depth("depth")
    .uniform_of("Camera")
    .instanced("Transform", "model.obj")));
```

`SeInstance` and `SeUniform` are generated from the component layouts, so
adding a field to `data.rs` makes it appear in the shader. `model.obj` names an
asset, not a file: whichever `asset/*.so` fills the asset slot answers.

## Crates

| Crate | Role |
|---|---|
| `se-abi` | every `#[repr(C)]` type that crosses a boundary. No deps, no alloc |
| `se-macro` | `derive(Schema)`, `#[se::stage]`, `stages!` |
| `se` | what a module compiles against: the entry macros and `GraphBuilder` |
| `se-host` | loader, world (sparse sets), scheduler, `Ctl` vtable, bundle |
| `se-render` | wgpu graph executor, OBJ meshes, layout → shader attributes |
| `se-tui` | half-block presenter and the IDE panel |
| `se-cli` | the `shinra` binary |

The engine provides general tools. It has no camera, no sprite, no dialogue and
no physics — those are things games have, and games live in
[`shinra-examples`](../shinra-examples).

## Using it

```bash
cargo build
export SHINRA_ENGINE=$PWD

target/debug/shinra build ../shinra-examples/game1
target/debug/shinra run   ../shinra-examples/game1          # TUI
target/debug/shinra shot  ../shinra-examples/game1 -n 3 --size 160x90 > f.ppm
```

`run` gives you the IDE: the presented buffer as truecolour half-blocks, and
beside it the passes in execution order, the loaded modules, and a log. Every
render pipeline gets a TUI, because the terminal is a presenter like any other.

`esc` quits.

### No GPU?

wgpu will pick a software adapter. On Debian/Ubuntu:

```bash
apt-get install -y mesa-vulkan-drivers libvulkan1
```

This is a supported configuration, not a fallback — the engine is headless by
design and never opens a window. `docker compose up` builds an image with it
already installed.

### When a pass misbehaves

```bash
SE_DUMP_WGSL=/tmp/wgsl target/debug/shinra shot <bundle> -n 1
```

writes each pass's fully composed shader, prelude included.

## Notes from building it

- **`dlopen` caches by path.** Reloading the same path silently returns the old
  code, so every load goes through a fresh shadow copy.
- **Nothing outlives its image.** The host deep-copies every string and byte at
  registration, so only function pointers still point into a module — which is
  why a module and the specs taken from it are dropped together.
- **A `.so` loses its statics when it is swapped.** State that must survive is a
  component. The examples follow this rule and so should you.
- **Uniform fields arrive as `vec4`.** WGSL aligns `vec3` to 16 bytes and
  `#[repr(C)]` aligns it to 4, so the host repacks rather than let the two
  quietly disagree. Read `u.eye.xyz`, `u.fov.x`.
- **A sampled buffer always reads the previous frame.** That is what keeps the
  graph acyclic: a mirror showing a mirror is a one-frame delay, not a cycle.
  It also means a multi-pass chain is one frame behind per hop.
- **Meshes are normalized to a unit box.** Otherwise swapping `bunny.so` for
  `teapot.so` would change the size of the world, and the two would not really
  be interchangeable. Scale belongs to `Transform`.

## Licence

See [LICENSE](LICENSE).
