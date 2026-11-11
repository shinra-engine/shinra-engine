# Changelog

`Cargo.toml` itself has stayed at `0.1.0` since the engine's current
workspace was cut (commit `3f565ae`, 2026-09-01) — nothing has been formally
released. The versions below are introduced by this changelog to label the
*entire* commit history, from the first commit (`25d0564`, 2026-03-09) to
`HEAD`, so the shape of the project — including the parts that were later
thrown away — stays legible. Numbering is sequential by era, not copied from
the repo's own ad hoc checkpoint commits (`v0.21.0`, `v0.22.0`, `v23`, `v24`
— these exist in the git log at different points and are called out by hash
below where relevant; any resemblance to this changelog's own numbers is
coincidental). Dates are the commits' own. Newest first.

## [Unreleased]

Nothing pending beyond 0.24.0 — this is the tip of `HEAD`.

---
## Era 3 — post-MCDV feature work (2026-11-03 – now)

The MCDV rewrite (Era 2, below) reaches today's `origin/main` at `d51b78a`
(2026-10-14). Everything from here on builds features on top of a settled
architecture.

## [0.24.0] — 2026-11-07 to 2026-11-10 — Hardening pass

### Fixed
Closes gaps opened by 0.18.0–0.23.0:
- A wasm32 compile error for an omitted `last_sound_seq` (`task-284`).
- `render::BLANK`'s `sample = 0` collided with `bind_for`'s "no sample"
  sentinel (`task-285`).
- `WebMem::data()`'s double-buffer aliasing reviewed for soundness
  (`task-286`).
- `WebMem::read`/`write` had no bounds check, unlike the native path
  (`task-287`).
- `render_worker` only rolled back on `Timeout`, not other replay failures
  (`task-288`).
- `touch.rs`'s `mouse_down` could stay stuck set across a Play→Edit switch
  without an intervening release event (`task-289`).
- `depth_view` panicked instead of returning an error on an unresolved
  offscreen id (`task-290`).
- The run loop's picture buffer could go stale/mismatched for a frame right
  after a resize (`task-291`).
- Duplicated `check_contract_id` logic (native vs. web) and duplicated,
  inconsistently hyphen-normalized control-var validation (`control.rs`'s
  four setters, which could silently fail to match a hyphenated WIT field
  name for `clock`/`sound` but not `input`/`pad`) both consolidated into one
  checked path (`task-292`–`task-294`).

### Documentation
- `engine.wit`, `command.rs` and the README's "Known limits" still
  described multi-pass fields as unimplemented after 0.18.0 had shipped
  them; corrected to match actual replay behavior.

## [0.23.0] — 2026-11-06 — Render thread

### Changed
- Rendering moved off the simulation thread: `render_worker.rs` runs the
  single `Gpu` on its own OS thread; the run loop sends it a cloned
  per-frame snapshot (never live memory) over a non-blocking channel and
  polls for the last finished picture. Two tests prove the simulation's
  clock sequence stays bit-identical regardless of render-thread lag.
  GPU-timeout recovery is now wired through this channel (`task-246`–
  `task-249`).

## [0.22.0] — 2026-11-06 — Sound

### Added
- A `sound { seq, id, gain }` control var injected into every contract via
  `shinra:engine`; `Vm::take_sound()` edge-detects `seq`; `se-cli`'s
  `audio.rs` resolves `id` to an asset blob and hands it to a headless log
  sink (no real audio device wired up yet) (`task-265`, `task-267`–
  `task-269`).

## [0.21.0] — 2026-11-06 — Touch input

### Added
- Touch/drag-gesture input: `se-tui`'s `Pointer` state machine tracks
  Down/Drag/Up/Moved from the terminal's existing mouse stream; `se-cli`'s
  `touch.rs` normalizes it into the existing `Keys.mouse`/`mouse_down`
  fields — no contract change — gated so a press counts as "down" only if
  it started inside the viewport in Play mode; wired into both the IDE and
  headless `shot --touch` (`task-250`–`task-254`).

## [0.20.0] — 2026-11-06 — Frame history

### Added
- Per-target one-frame history: each offscreen target keeps a prev/cur
  texture pair, copied at the end of a frame; a draw with `FLAG_PREV` reads
  last frame's content for that target instead of this frame's, enabling
  trail/feedback effects (`task-263`, `task-264`, `task-266`).

## [0.19.0] — 2026-11-05 — Web host target

### Added
- A second compile target for the host itself: `se-vm`'s module loader and
  linear-memory access go behind a `Mem` trait, with `NativeMem`/wasmtime
  unchanged and a parallel `WebMem` over browser `WebAssembly.Memory`
  (`web-sys`); `module::native`/`module::web` reimplement the same
  import/shape/`CONTRACT_ID` checks through JS reflection on the web path
  (`task-239`, `task-256`–`task-258`, story-229).

### Known gaps at this version (closed in 0.24.0 where noted)
- No fuel-budget enforcement on the web path yet — a runaway stage can only
  hang the tab, not be cut off and rolled back the way a native trap is.

## [0.18.0] — 2026-11-05 — Multi-pass rendering

### Changed
- **Breaking.** `render::command` widened from 52 to 64 bytes to carry
  `target`, `sample`, `render-flags` — every compiled game's `CONTRACT_ID`
  changes; existing games must rebuild (`task-238`).

### Added
- Offscreen render targets `1..=8`, created lazily at the present target's
  size; `replay_inner` groups a frame's command list per pass; a
  `DRAW_FULLSCREEN` with `sample` set composites an earlier pass's target
  (`task-270`–`task-273`).
- Depth attachments: `BEGIN_PASS.render-flags` bit 0 (`FLAG_DEPTH`) attaches
  and clears a depth buffer, so 3D bodies occlude correctly within a pass
  (`task-274`, `task-275`).
- A GPU wall-clock budget: `device.poll` is bounded to 2s (`GPU_BUDGET`); a
  timeout surfaces as a reported error, and the CLI rolls the render module
  back to the last-good one — the same rollback path already used for a
  trapping module (`task-242`–`task-244`).

## [0.17.0] — 2026-11-03 — Render budget + gamepad

### Added
- Render-budget skipping: `Gpu` measures its own replay+readback cost; the
  run loop skips a frame's render and reuses the last picture when over
  budget, independent of the simulation step (`task-225`, `task-226`; pinned
  by a test that the fixed-step clock sequence is unaffected, `task-227`).
- Gamepad input: a `pad` control var mirroring `input`, `Vm::set_pad`, a
  `PadSource` trait and `ScriptedPad` for deterministic end-to-end tests
  (`task-230`–`task-232`).

---
## Era 2 — the MCDV rewrite (2026-08-27 – 2026-10-14)

The pre-rewrite engine (Era 1) ends at `b18a4b9`. Everything in this era
replaces it with the architecture the current README/design.md describe:
four wasm module kinds sharing one host-owned arena under `contract.wit`.

## [0.16.0] — `008e35e`..`d51b78a`, 2026-10-13 to 2026-10-14 — Textures and pair stages

- OBJ/PNG decoded into real GPU resources with texture binding;
  `render.command` gains `mesh`/`texture`; replay does N instances at their
  own offsets, one pipeline per column (`task-204`–`task-207`).
- A second example game samples a sprite atlas (`task-209`).
- `#[se::pair]` lands — stages over pairs of rows, with a v2 stage table
  (`task-211`).
- README documents clock, contract id, assets, pair stages (`task-214`).
- Two closing fixes: a zero-instance GPU draw, and the editor initializing
  games before it opens. Lands exactly at `origin/main` `d51b78a`.

## [0.15.0] — `49f8080`..`f75e06e`, 2026-10-13 — `shinra:engine` + determinism

- README rewritten to describe MCDV; `design.md` trimmed to a terse
  "deferred items" doc (`49f8080`).
- `se-contract` injects a second, engine-owned WIT package (clock, input,
  command, frame) that every game reuses rather than restates (`task-195`,
  `task-196`).
- `control.clock` is written host-side before every tick at a fixed step;
  games stop writing their own clock (`task-197`, `task-198`).
- `se-vm`/the CLI start refusing a module built against a mismatched
  contract — the `CONTRACT_ID` check (`task-201`).
- **Why this is a boundary**: this is the period that actually delivers the
  README's "two runs fed the same input are byte-identical" and
  "any module following the spec can be hot-replaced" guarantees as a single
  shared spec, not per-crate convention.

## [0.14.0] — `b61301f`..`3f1cf1a`, 2026-10-06 — Hot-reload + rollback

- `world::place` refuses a row capacity whose columns overflow `u32`
  (`task-181`).
- `Reloader::plan` decides Nothing/Restart/Modules for a source edit;
  `Reloader::apply` recompiles and swaps a stale game/render module, then
  process stages and asset blobs; `shinra run` polls every 250ms,
  `--no-watch` turns it off (`task-183`–`task-187`).
- Game/render exports are checked before instantiation; process stages are
  checked before instantiate, with statics surviving a refusal
  (`task-172`, `task-173`).
- A trapping game/render swap rolls back to the previous module; a trapping
  process swap rolls back stages and statics too (`task-174`, `task-175`).
- `se-tui` log lines carry a level, multi-line errors split/cap/draw red;
  reload failures become deduplicated values, surfaced red, and the IDE
  stays in Play after a rollback (`task-176`–`task-179`).
- **Why this is a boundary**: this period alone delivers README's "rollback
  restores modules, not the world" as working code with tests, not just a
  design statement.

## [0.13.0] — `f63978c`..`7c85c5c` / wasm-target discipline, 2026-10-02 to 2026-10-06 — Test hygiene + CI

- `scripts/ci.sh` introduced: build/test the live engine crates, then build
  examples and validate shots (`task-158`, `task-159`).
- An absurd draw count is refused by a capacity check (se-gpu).
- Every crate's tests stop silently skipping without the wasm32 target — a
  shared `require_wasm_target` helper panics instead, across se-vm,
  se-contract and se-gpu (`task-166`–`task-170`).
- **Why this is a boundary**: a policy pass, not a feature — the project
  stops tolerating tests that quietly pass by doing nothing.

## [0.12.0] — `2edc251`..`7de80b7`, 2026-10-02 — IDE catches up

- Inspector decodes a typed field from arena bytes and commits an edit back
  through a real `Vm` write, driven through Enter/typing in the IDE loop
  (`task-124`–`task-127`).
- Project panel rows carry module identity and an active marker; selection
  gets focus/Up/Down/Enter; Enter on a module swaps it into the running Vm
  (`task-128`–`task-131`).
- **Why this is a boundary**: editor-only work riding on top of the engine
  core the surrounding periods are building — not engine-core itself.

## [0.11.0] — `22ac6e1`..`42e426d`, 2026-09-30 — WIT becomes the only spec

- `se-abi`, `se-host`, `se-render` deleted from the workspace outright, now
  redundant with `se-vm`/`se-contract`; dead `frontend/` and `web/` (plus the
  Tauri/Playwright GUI compose service) deleted (`task-82`, `task-83`,
  `task-85`, `task-86`).
- `derive(Schema)`/`components!` Rust macros replaced by `contract.wit`
  itself, enforced as compile errors; generated names made consistently
  `snake_case`; the stage macro names components the way the contract does,
  with an end-to-end test for multi-word names (`task-89`–`task-92`).
- Contract records gain variable-length `span`/`string`/`list` fields,
  resolved through host-owned blob slots the host fills and modules reuse
  (`task-110`–`task-113`).
- **Why this is a boundary**: this is where "nothing hand-duplicates the
  contract" — generated, never restated — takes hold as an enforced rule
  rather than a convention.

## [0.10.0] — `65c5c30`/`68363b6`/`5152828`, 2026-09-11 — Pick a winner

- Task-40's own commit message: *"v24 carried this branch's additions but
  not its removals, so both routes were in the tree at once and neither
  compiled."* Deletes `se-ecs`, `se-wasm`, `se-web`, and the `.so`/native
  control route in `se-abi` outright — 45 files, almost no additions, 6845
  lines removed. Only the wasm-via-`se-vm` route survives.
- The keyboard becomes a control var; a draw declares `vertices` explicitly
  (`task-41`).
- The IDE itself is moved onto the surviving wasm route (`task-42`).
- **Why this is a boundary**: this is where MCDV's actual lineage starts —
  everything after this builds on the one route that survived the 0.9.0
  spike, not the two that were cut.

## [0.9.0] — `d2c76f6` "v24", 2026-09-11 — Three-way architecture spike

- The single largest commit in the repository: 126 files, +14139/-1971.
  Stands up *three competing backends side by side*: `se-ecs` (a native
  ECS), `se-wasm` (wasm called through a host ABI), and `se-web` (wasm run
  through a JS/web route, with its own `web/` scaffold and a
  `routes_agree.sh` parity test across them).
- Also introduces the first `se-contract` (a WIT→Rust/WGSL compiler),
  `se-vm` (the wasmtime host: module/memory/vm/world/stages), and `se-gpu`
  (command list + replay) — the crates that are still here today.
- Writes `docs/wasm-spike.md`.
- **Why this is a boundary**: explicitly an open experiment — three
  unfinished routes committed at once, not yet a decision about which one
  the engine would become.

## [0.8.0] — `3f565ae` "v23", 2026-09-01 — Hard reset

- One commit, 103 files, +7462/-8205. Deletes the old top-level `engine/`,
  `scene/`, `abi/`, `cli/` crates wholesale — the entire ECS scene engine,
  collision, sprite, presenter/window, script code gone — and stands up a
  brand-new `crates/` workspace skeleton: `se-abi`, `se-cli`, `se-host`,
  `se-macro`, `se-render`, `se-tui`, `se`, still named around "abi/host," not
  yet "wasm module."
- README and design.md rewritten to match the new skeleton.
- This is also the commit that first gives `Cargo.toml` a `version` field
  at all, set to `0.1.0`.
- **Why this is a boundary**: the literal gut-and-restart point — everything
  before this commit is a different engine.

## [0.7.0] — `38d93f3`..`b18a4b9` "v0.21.0"/"v0.22.0", 2026-08-27 — Last pre-rewrite checkpoint

- The engine author's own two checkpoint commits marking the final state of
  the pre-rewrite architecture (Era 1) before the hard reset in 0.8.0.

---
## Era 1 — pre-rewrite: scene-based ECS engine (2026-03-09 – 2026-08-09)

A different engine entirely: a scene-based ECS with an egui-then-Tauri+Svelte
GUI editor and a ratatui TUI, with TUI/GUI parity as an explicit goal. None
of this architecture survives into Era 2 — it is recorded here so the
project's full lifetime is legible, not because any of it is still live.

## [0.6.0] — `70bde85`..`b279166`, 2026-06-11 to 2026-08-09 — Content on a stable architecture

- GPU text-art viewport for the TUI: Mixed/Quadrant/Braille render modes.
- Sprite rendering, a "run mode," and a `dino-run` example game (game3).
- Dialogue overlays and a galgame-style text flow, with new TUI E2E
  coverage.
- **Why this is a boundary**: after five periods of infrastructure and
  pivots, this is the first stretch that's purely additive content work on
  an architecture that had stopped changing shape.

## [0.5.0] — `11d5e39`..`35c17ad`, 2026-05-30 — GUI rewritten egui → Tauri+Svelte

- `tui/`/`gui/` moved under `frontend/` for a clearer split; a GUI snapshot
  fix uses a real `cube.obj` fixture instead of a placeholder.
- The egui GUI is deleted wholesale (net -2166 lines) and replaced by a
  Tauri+Svelte scaffold, specifically to get Playwright E2E screenshots;
  Docker Compose added for GUI dev + full-IDE Playwright E2E.
- **Why this is a boundary**: a hard GUI-toolkit swap, same motivation
  (testability) as the test-infra period before it.

## [0.4.0] — `4741ce5`..`53b880e`, 2026-05-30 — Scene persistence + TUI/GUI parity proof

- `Scene::save()`/`load()` added as the one persistence path; both the TUI
  (`Ctrl+S`) and the GUI editor call it instead of separate save logic.
- E2E tests for both frontends — TUI via `TestBackend` text snapshots, GUI
  via PNG framebuffer capture — capped by a parity test asserting TUI and
  GUI produce byte-identical `scene.ron` for the same actions.
- Baseline snapshots stored and consolidated into one workspace-level
  directory.
- **Why this is a boundary**: the feature-complete milestone for this era's
  stated goal — "TUI and GUI are provably equivalent."

## [0.3.0] — `132d9b8`..`805970e`, 2026-05-30 — Snapshot/pixel-diff test infrastructure

- Editor-side dead code removed (`mesh_cache`, ad hoc scene conversion),
  replaced with the same `EngineBackend` calls the TUI already used.
- Snapshot testing lands: a render-to-PNG helper, pixel-diff comparison
  with tolerance, baseline PNGs, an `UPDATE_SNAPSHOTS` env var to refresh
  them.
- A frontend-parity integration test, and a fix to skip tests when external
  assets are missing.
- **Why this is a boundary**: from here on, visual regressions are caught
  automatically instead of by eye.

## [0.2.0] — `ac1e399`..`0da0351`, 2026-05-30 — `EngineBackend` abstraction

- `"TUI"` deletes the entire `vscode-ext/` (500+ lines) and introduces a
  ratatui TUI in its place — a hard pivot in tooling direction.
- Introduces the `EngineBackend` trait plus `frame_rgba`/`frame_image`/
  `load_scene`/`render_current`/`snapshot` methods — the engine gains a
  stable, backend-agnostic rendering/readback API.
- Viewport code in both the TUI and the editor is rewritten to delegate to
  this new API instead of duplicating readback logic.
- **Why this is a boundary**: "two frontends talk to one engine API" is
  established here and holds for the rest of the era.

## [0.1.0] — `25d0564`..`04f984e`, 2026-03-09 to 2026-05-16 — Scaffold + VS Code extension

- `25d0564` "Initial commit", then nothing until 2026-05-11: the engine
  workspace is imported from a `shinra-examples` split, with `scene`/
  `editor-server` crates and a 500+-line VS Code extension for project
  tooling.
- `editor-server`: a scene-based "game cycler" driving two HTTP viewports —
  game on :5813, editor on :5814 — plus a dev/release Docker split.
- README rewritten to describe this scene-based model and a roadmap.
- **Why this is a boundary**: pure scaffold and tooling, nothing playable
  yet — and the VS Code-extension direction is abandoned in the very next
  period.
