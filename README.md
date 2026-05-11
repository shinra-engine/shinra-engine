# shinra-engine-core

The Rust + wgpu game engine plus its tooling: render core, scene types, two
front-ends (`runner` for terminal, `editor` / `editor-server` + VS Code
extension for the GUI viewport), and the build infrastructure for games.

Game **data** lives in a separate project (e.g.
[`shinra-examples`](../shinra-examples/)) — a folder per game holding
`scene.ron` + `tscn.ron`. The editor-server scans that folder, renders the
selected game, and **`n`** in the viewport cycles to the next.

## Two coexisting game models

This repo currently supports two ways of describing a game; they're being
unified, not maintained in parallel forever.

| Model | Lives in | Loader | Status |
|---|---|---|---|
| **scene-based** (data) | `<project>/assets/games/<name>/{scene.ron,tscn.ron}` | `editor-server` (working) | Current direction. |
| **cdylib** (code) | `<project>/games/<name>/` Rust crate, compiled to `libgame*.so` via `.hom` DSL → `homunc` → rustc | `runner` (`target/debug/libgame*.so`) | Legacy. The build infra (`hom_hecs` runtime, `homunc` integration, build.rs templates) belongs in this repo so projects don't carry it; see "Roadmap" below. |

We call the architecture **gametok**: TikTok-style swipe between games. `n`
is consumed by the loader, never seen by the game.

## Prerequisites (Ubuntu / Debian, only if building natively)

If you only run the editor-server through Docker (the recommended path), you
don't need any of this on the host. For native builds:

```bash
# Rust toolchain — current stable (1.88+ required by wgpu 27)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
. "$HOME/.cargo/env"

# C/C++ toolchain + openh264 build deps (needed by editor-server)
sudo apt install -y build-essential cmake nasm pkg-config

# Vulkan runtime for headless wgpu rendering (editor-server / editor)
sudo apt install -y mesa-vulkan-drivers libvulkan1 vulkan-tools

# Node.js 20+ (only needed to package the VS Code extension in vscode-ext/)
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
sudo apt install -y nodejs
```

## Run natively

```bash
cargo build                        # builds engine, scene, runner, editor, editor-server
cargo run -p editor-server         # HTTP :5812 + WS :5813 (H.264 stream)
cargo run -p editor                # native egui editor
cargo run -p runner                # terminal mode; cycles libgame*.so in target/debug
```

The editor-server resolves asset paths relative to its current working
directory, so run it from inside a game project (or use Docker, which mounts
the project at `/game`).

## Build the editor-server Docker image

The editor-server is the entry point for VS Code's viewport. Game projects
launch it via their own `docker-compose.yml` (which bind-mounts the project
as `/game` and this repo as `/engine-core`). Two images are available:

- **`Dockerfile`** (dev) — keeps the full Rust toolchain inside the
  container. `cargo run -p editor-server` runs at container start against the
  source mounted at `/engine-core`, so engine edits rebuild on
  `docker compose up` without rebuilding the image.
  ```bash
  docker build -t shinra-editor-server .
  ```
- **`Dockerfile.release`** (slim) — multi-stage build that bakes the release
  binary into a `debian:bookworm-slim` runtime. Smaller image, faster start,
  but engine source changes require an image rebuild.
  ```bash
  docker build -f Dockerfile.release -t shinra-editor-server .
  ```

Both images use Rust **1.88-bookworm** (required by wgpu 27 and the
edition-2024 crates in `Cargo.lock`), set `WORKDIR /game`, and ship
`mesa-vulkan-drivers` so wgpu renders headlessly via lavapipe with no GPU.

`shinra-examples/docker-compose.yml` references the image by name and works
with either build.

## VS Code extension (live viewport)

`editor-server` exposes the rendered scene as an H.264 WebSocket stream that
the extension in `vscode-ext/` decodes inside a VS Code webview.

```bash
cd vscode-ext
npm install
npm run compile                    # tsc → out/extension.js
```

Then either:

- **Dev (Extension Development Host):** `code vscode-ext`, press **F5**. A
  second VS Code window opens with the extension loaded. Run
  `Ctrl+Shift+P` → **Shinra: Open Viewport**.
- **Permanent install:**
  ```bash
  npm run package                              # produces shinra-editor-*.vsix
  code --install-extension shinra-editor-*.vsix
  ```

`editor-server` must already be running (Docker or native) — the extension is
just a viewer/client talking to `:5812` (HTTP scene API) and `:5813` (WS
frame stream).

| Key (in the viewport) | Action |
|---|---|
| Arrow keys | move node 0 in screen-X / screen-Y |
| **n** | cycle to next game |

## Workspace layout

```
shinra-engine-core/
├── engine/         shinra-engine — wgpu device, render pipeline, presenter, Keymap
├── abi/            gametok-abi   — #[repr(C)] InputFrame, Drawable (cdylib FFI)
├── scene/          serde scene + camera types (scene.ron / tscn.ron)
├── runner/         terminal binary; dlopen + render loop + n-swipe
├── editor/         native egui editor (eframe + wgpu)
├── editor-server/  HTTP :5812 + WS :5813 H.264 stream — scene-based loader
├── vscode-ext/     VS Code extension (decodes the H.264 stream)
├── Dockerfile      dev image (cargo at container start)
└── Dockerfile.release  slim multi-stage runtime image
```

## The gametok cdylib FFI (legacy)

A cdylib game must export five C symbols. The runner dlopens it, calls
`tick(dt, input)` per frame, and reads `drawables_ptr` / `drawables_len`.

```rust
extern "C" fn meshes_count() -> u32;
extern "C" fn meshes_path(i: u32, out: *mut u8, cap: u32) -> u32;
extern "C" fn tick(dt: f32, input: *const InputFrame);
extern "C" fn drawables_ptr() -> *const Drawable;
extern "C" fn drawables_len() -> u32;
```

`Drawable { mesh_id, model: [f32; 16] }` — column-major mat4. The runner
copies into a transient `Scene`, calls `engine.render(&scene)`, and presents.
Each game's `hecs::World` lives in its own `thread_local!`, so swiping
between games is a clean state reset — nothing leaks across `dlclose`.

The `.hom` DSL → `homunc` → rustc cdylib pipeline that produced these
formerly-shipped in `shinra-examples/games/`. It still exists; the build
infrastructure (templates, `hom_hecs` runtime, `homunc` invocation) needs to
be relocated into this repo and exposed via the editor (e.g. as a "scaffold a
new cdylib game" command). See "Roadmap".

## Tests

```bash
cargo test                  # unit + render smoke test
ls target/debug/smoke/      # cube.png teapot.png bunny.png — sanity render outputs
```

## Roadmap

1. **Runner reads scene-based games.** Today `runner` only knows about
   `target/debug/libgame*.so`. Teach it to also (or instead) cycle
   `assets/games/*/scene.ron` so editor and runner share one game model.
2. **Move cdylib build infra into this repo.** `homunc`, `hom_hecs/`, and
   the per-game `build.rs` template currently expect to live next to game
   source. Lift them into `engine-core` and let the editor "scaffold game"
   command stamp out a new game folder against this repo's templates.
3. **Save edits from the viewport.** Arrow-key translations are in-memory
   only; add an explicit save key (or HTTP endpoint) that writes the current
   game's `scene.ron` / `tscn.ron` back to disk.

## Status

POC complete: the editor-server cycles scene-based games (`game1` bunny,
`game2` teapot) over the H.264 WS stream consumed by the VS Code extension.
The native runner still loads cdylib `.so` files only.
