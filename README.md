# shinra-engine-core

The Rust + wgpu game engine. Games are **runtime-loaded `.so` plugins**: this
binary never recompiles when a game changes — drop a new `libgameN.so` into
`target/debug/` and it shows up on the next swipe.

Sample games live in the sibling repo
[`shinra-examples`](../shinra-examples/) — clone it next to this directory so
the games' `Cargo.toml` path deps to `abi/` and `scene/` resolve.

We call the architecture **gametok**: TikTok-style swipe between games. Press
`n` and the current `.so` is unloaded, the next one loaded.

```
.hom (game DSL)              ┐
   │ homunc                  │  per-game artifact
   ▼                         │  (rebuilt only when the game changes)
generated .rs                │
   │ rustc cdylib            │
   ▼                         │
libgameN.so   ←  exports `tick(dt, input)` + `drawables_ptr()`  ┘

shinra runner  ←  stable wgpu + wgsl + viuer/window core
   ├─ scans target/debug for libgame*.so
   ├─ dlopen one game at a time
   ├─ Keymap → InputFrame → tick(dt, &input)
   ├─ pulls drawables, builds a Scene, renders
   └─ `n` → drop(lib), load next
```

See [`design.md`](design.md) for the architecture rationale.

## Prerequisites (Ubuntu / Debian)

```bash
# Rust toolchain — apt's cargo is too old for Cargo.lock v4
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
. "$HOME/.cargo/env"

# C/C++ toolchain + openh264 build deps (needed by editor-server)
sudo apt install -y build-essential cmake nasm pkg-config

# Vulkan runtime for headless wgpu rendering (editor-server / editor)
sudo apt install -y mesa-vulkan-drivers libvulkan1 vulkan-tools

# Node.js 20+ (only needed to build/package the VS Code extension in vscode-ext/)
# Apt's nodejs on Ubuntu 24.04 is too old for `@vscode/vsce` (undici requires Node >= 20).
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
sudo apt install -y nodejs
```

`runner` alone (terminal mode) only needs the Rust toolchain. The Vulkan stack is required for `editor-server` and the egui `editor` because both rely on a real wgpu adapter; mesa includes lavapipe as a software fallback when no GPU is present. Node.js is only required if you intend to build the VS Code extension.

## Run

```bash
cargo build                        # builds engine, runner, both games
cargo run -p runner                # cycles libgame*.so in target/debug
cargo run -p editor-server         # HTTP :5812 + WS :5813 (H.264 stream)
```

### VS Code extension (live viewport)

`editor-server` exposes the rendered scene as an H.264 WebSocket stream that the
extension in `vscode-ext/` decodes inside a VS Code webview.

```bash
cd vscode-ext
npm install
npm run compile                    # tsc → out/extension.js
```

Then either:

- **Dev (Extension Development Host):** `code vscode-ext`, press **F5**. A second
  VS Code window opens with the extension loaded. Run command palette
  (`Ctrl+Shift+P`) → **Shinra: Open Viewport**.
- **Permanent install:**
  ```bash
  npm run package                              # produces shinra-editor-*.vsix
  code --install-extension shinra-editor-*.vsix
  ```

`editor-server` must already be running — the extension is just a viewer/client
talking to `:5812` (HTTP scene API) and `:5813` (WS frame stream).

| Key | Action |
|---|---|
| W A S D | move object (XZ plane) |
| ← → ↑ ↓ | rotate object (yaw / pitch) |
| j / k | enlarge / shrink |
| **n** | **swipe to next game** (drops `.so`, loads next) |
| q / Esc | quit |

`n` is consumed by shinra — games never see it. All other keys are passed in
as a semantic [`InputFrame`](abi/src/lib.rs) so games don't deal with raw key
codes.

## Workspace layout

```
shinra-engine-core/
├── engine/         shinra-engine — wgpu device, render pipeline, presenter, Keymap
├── abi/            gametok-abi   — #[repr(C)] InputFrame, Drawable (shared FFI)
├── runner/         runner bin    — stable; dlopen + render loop + n-swipe
├── scene/          scene graph types
├── editor/         egui-based editor
├── editor-server/  HTTP :5812 + WS :5813 H.264 stream
└── vscode-ext/     VS Code extension (decodes the H.264 stream)

shinra-examples/    (sibling repo)
├── games/
│   ├── game1/      bunny  (.hom)
│   ├── game2/      teapot (.hom)
│   └── game3/      .hom example
└── assets/         bunny.obj, teapot.obj
```

The runner scans `target/debug/` for `libgame*.so`; build games in the
examples repo and the resulting `.so` files land in `shinra-examples/target/debug/`.
Symlink or copy them into this repo's `target/debug/`, or point the runner at
the examples target dir.

## Creating a mini-game

Games live in the [`shinra-examples`](../shinra-examples/) repo; see its
README for the full mini-game scaffolding flow. The summary:

A game is a Rust **cdylib** that exports the [gametok FFI](abi/src/lib.rs).
You can write the gameplay in either:

- **`.hom`** — Homun DSL, compiled by `homunc` to Rust at build time. Ergonomic
  for ECS-style game logic (this is what `game1` and `game2` use).
- **Pure Rust** — direct `hecs::World` + plain functions. More boilerplate;
  useful when you need crate dependencies the DSL doesn't expose.

The fastest path is to copy `games/game1/`, rename, swap the mesh, tweak
`main.hom`. Concrete steps:

### 1. Drop in the `homunc` compiler (one-time, only if writing `.hom`)

Download the latest `homunc` Linux x86_64 binary from
[homun-lang/homun releases](https://github.com/homun-lang/homun/releases) and
place it at `.tmp/homunc`:

```bash
mkdir -p .tmp
curl -L https://github.com/homun-lang/homun/releases/latest/download/homunc-linux-x86_64 \
  -o .tmp/homunc
chmod +x .tmp/homunc
```

`build.rs` will find it automatically.

### 2. Scaffold the new game

```bash
cp -r games/game1 games/game3
sed -i 's/name = "game1"/name = "game3"/' games/game3/Cargo.toml
```

Add the new crate to the workspace in the root `Cargo.toml`:

```toml
[workspace]
members = ["engine", "abi", "runner", "games/game1", "games/game2", "games/game3"]
```

### 3. Tell it which mesh to load

Edit `games/game3/src/lib.rs`:

```rust
const MESH_PATHS: &[&str] = &["assets/your_mesh.obj"];
```

Drop the `.obj` file into `assets/`. Multi-mesh games declare more paths;
each `Drawable.mesh_id` indexes this slice.

### 4. Write the gameplay (`games/game3/src/main.hom`)

The contract: `main_tick(dt, input)` runs once per frame. Use `hom_hecs`
helpers (`spawn1`/`query1`/`query1_mut`) to build and step your hecs World.
After `main_tick` returns, `lib.rs` walks the World and pushes a `Drawable`
per renderable entity into the per-frame buffer the runner reads.

Minimal example — a single object that moves in response to `InputFrame`:

```
use std
use hom_hecs

@derive(Clone, Debug)
Player := struct { x: float, y: float, z: float, yaw: float, pitch: float, scale: float }

main_tick := (dt: float, input: InputFrame) -> _ {
  spawned := false
  query1((p: Player) -> _ { spawned := true })
  if (not spawned) {
    spawn1(Player { x: 0.0, y: 0.0, z: 0.0, yaw: 0.0, pitch: 0.0, scale: 1.0 })
  }

  query1_mut((p: Player) -> Player {
    Player {
      x:     p.x + input.move_x * 2.0 * dt,
      y:     p.y,
      z:     p.z + input.move_z * 2.0 * dt,
      yaw:   p.yaw   + input.rot_yaw   * 1.5 * dt,
      pitch: p.pitch + input.rot_pitch * 1.5 * dt,
      scale: scale_step(p.scale, input.scale_delta, dt),
    }
  })
}
```

`InputFrame` fields are `move_x`, `move_z`, `rot_yaw`, `rot_pitch`,
`scale_delta` — see [`abi/src/lib.rs`](abi/src/lib.rs).

### 5. Build and swipe to it

```bash
cargo build -p game3      # produces target/debug/libgame3.so
cargo run -p runner       # press n until you hit your game
```

The runner is **not rebuilt**. Adding `game4` later? Same flow — runner picks
up new `.so` files in sorted order on next launch. Removing one (`mv
target/debug/libgame3.so /tmp/holdout`) makes it disappear from the swipe
cycle without any code changes.

## The FFI contract

Every game `.so` must export six C symbols. `lib.rs` in the game template
provides all of them; you only ever change `MESH_PATHS` and the body of
`main_tick`.

```rust
extern "C" fn meshes_count() -> u32;                              // how many meshes the game wants loaded
extern "C" fn meshes_path(i: u32, out: *mut u8, cap: u32) -> u32; // utf-8 path written into out
extern "C" fn tick(dt: f32, input: *const InputFrame);            // advance one frame of game state
extern "C" fn drawables_ptr() -> *const Drawable;                 // pointer to this frame's draw list
extern "C" fn drawables_len() -> u32;                             // length of draw list
```

`Drawable { mesh_id, model: [f32; 16] }` — column-major mat4. The runner
copies into a transient `Scene`, calls `engine.render(&scene)`, and presents.

Pointers stay valid until the next `tick`. Each game's `hecs::World` lives in
its own `thread_local!`, so swiping between games is a clean state reset —
nothing leaks across `dlclose`.

## Tests

```bash
cargo test                  # 14 unit + 1 render smoke test
ls target/debug/smoke/      # cube.png teapot.png bunny.png — sanity render outputs
```

## Status

POC complete. The runner cycles `game1` (bunny) and `game2` (teapot) without
recompilation. Out of scope for now: hot-reload via filesystem watcher,
save/restore across swipes, window mode (terminal/viuer only), input event
plumbing for held keys (terminals only deliver press events — each frame
treats keys as one-shot impulses).
