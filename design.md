# Shinra Game Engine — Design

## Goal

A wgpu-based game engine that renders to an off-screen buffer, with multiple
pluggable presenters:

1. **Window presenter** — blit the buffer to a native window surface.
2. **Terminal/ASCII presenter** — read buffer back to CPU, map to ASCII, write
   to terminal (playable inside tmux).

Future presenters are possible (headless capture, web, etc.).

## Architecture

```
[Scene] → [Renderer: wgpu → offscreen texture] → [Presenter]
                                                   ├─ Window (surface blit)
                                                   └─ Terminal (readback → ASCII)
```

- The renderer never talks directly to a window. It always produces a texture.
- Presenters are swappable; the engine can run with one or both active.

## 2D / 3D — Unified Pipeline

Decision: **one pipeline, two configurations**, not two pipelines.

- Core pipeline is the same: vertex → fragment → depth test → blend.
- 2D and 3D differ only in:
  - **Projection matrix** — orthographic (2D) vs perspective (3D). Per-camera uniform.
  - **Depth buffer** — used by both. 2D layers are just Z values.
  - **Blend / cull state** — cheap pipeline state swap, not a separate pipeline.
- Scene model: drawables with a transform (including Z) + a camera that picks
  projection. A 2D game is "orthographic camera + quads at different Z."

### Caveat
Transparent/translucent objects in 3D still need back-to-front sorting — this is
a sorting concern, handled in the scene layer, not a pipeline split.

### Bonus for ASCII presenter
Real depth buffer means the terminal view can sample depth alongside color for
fog/distance shading, which makes 3D scenes readable as text.

## Terminal / ASCII Presenter

### Library: **viuer**
- Rust-native terminal image lib.
- Supports Kitty graphics, iTerm2, Sixel, with **half-block truecolor** fallback.
- API takes `image::DynamicImage` — feeds naturally from a wgpu RGBA readback.
- Designed for static images; per-frame use is acceptable for MVP, revisit if
  bottlenecked.
- Fallback path if viuer becomes a bottleneck: chafa via FFI (full mode coverage,
  animation-friendly), or hand-rolled half-block + crossterm (~100 lines).

### Considerations
- **Per-frame GPU→CPU readback is required.** Stalls the GPU. Mitigate with a
  low-res render target (~120×40 cells) and a separate frame cadence from the
  window path.
- **Color** — TBD: monochrome luminance vs 24-bit truecolor (not universal).
- **Role** — TBD: first-class play target (drives input/pacing design) vs
  debug/novelty view (window drives design).

### TUI Chrome — ratatui?

`ratatui` (formerly `tui-rs`) is the standard Rust widget-TUI lib (panels,
tables, gauges, text). It is **not** an image renderer — it cannot display the
wgpu framebuffer on its own, and naive composition with viuer fights ratatui's
redraw loop.

**Decision: skip ratatui for MVP.** The terminal presenter is pure
framebuffer — `viuer` for image output, `crossterm` for raw mode + keypress
input. Rationale:

- The framebuffer *is* the content; no sidebars or tables to lay out.
- One rendering path (wgpu → readback → viuer), one input path (crossterm).
  No ratatui ↔ viuer redraw conflicts.
- Frame pacing stays explicit — terminal can't hit 60 fps and we want control
  over readback cadence.

Escape hatch: if a dev HUD with multiple panels (FPS, scene tree, logs)
becomes essential, migrate to the `ratatui-image` crate rather than gluing
viuer + ratatui by hand.

## MVP Implementation

Concrete decisions for the first POC. The MVP renders `assets/teapot.obj` and
`assets/bunny.obj` with a fixed orbit camera, viewable both in a native window
and inside a tmux pane.

### Crate stack

| Crate | Purpose |
|-------|---------|
| `wgpu` | GPU API |
| `winit` | Window + input event loop (window binary only) |
| `pollster` | Block-on async executor for wgpu init |
| `bytemuck` | POD casts for vertex/uniform buffers |
| `glam` | Matrix/vector math |
| `tobj` | OBJ mesh loader |
| `image` | RGBA buffer type consumed by viuer |
| `viuer` | Terminal image rendering (Kitty/Sixel/half-block) |
| `crossterm` | Terminal raw mode + keypress input |

Pin exact versions in `Cargo.toml`, not here.

### Module layout

```
Cargo.toml
src/
  lib.rs              # public re-exports
  engine.rs           # Engine: device/queue, offscreen target, render()
  scene.rs            # Scene, Camera, Drawable
  mesh.rs             # Mesh, Vertex, OBJ loader (tobj)
  shader.wgsl         # single pipeline shader
  presenter/
    mod.rs            # Presenter trait, FrameCtx
    window.rs         # WindowPresenter (winit + wgpu surface)
    terminal.rs       # TerminalPresenter (readback → viuer)
src/bin/
  window.rs           # cargo run --bin window
  terminal.rs         # cargo run --bin terminal
assets/
  teapot.obj
  bunny.obj
```

One library crate, two binaries. Each binary owns its own frame loop and runs
**one** presenter — we don't run window + terminal simultaneously in MVP.

### Renderer spec

- **Offscreen target**: single `wgpu::Texture`, `Rgba8UnormSrgb`, MVP size
  ~256×144 px (configurable). Owned by `Engine`.
- **Depth buffer**: `Depth32Float`, same size.
- **Pipeline**: one pipeline, sRGB color target, depth test enabled.

### Vertex format

```rust
#[repr(C)]
struct Vertex { position: [f32; 3], normal: [f32; 3] }
```

Indexed draws. `tobj` provides position + normal directly.

### Shader (MVP)

Single WGSL file. Lambert shading with one hard-coded directional light:

```
color_rgb = base * max(dot(normal_world, light_dir), 0.15)
```

`base` is a uniform constant for now (no per-mesh material). Good enough to
distinguish geometry; readable when downsampled to ASCII.

### Camera

Fixed orbit camera for MVP — auto-rotates around origin at constant speed,
fixed radius and pitch. Perspective for 3D, orthographic for 2D (selected per
scene). No user control yet.

### Presenter trait

```rust
pub struct FrameCtx<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub texture: &'a wgpu::Texture,  // the offscreen render target
    pub width: u32,
    pub height: u32,
}

pub trait Presenter {
    fn present(&mut self, ctx: &mut FrameCtx<'_>);
}
```

Each presenter pulls what it needs from `FrameCtx`:
- **WindowPresenter** does a GPU-side blit from `ctx.texture` to its surface.
- **TerminalPresenter** does a readback into a CPU `Vec<u8>`, wraps it in
  `image::RgbaImage`, hands to `viuer::print`.

### Frame loop

- **Window binary**: winit event loop. On `RedrawRequested` →
  `engine.render()` → `presenter.present()`. Driven by surface vsync.
- **Terminal binary**: tight loop with ~33ms sleep (≈30 fps cap).
  `engine.render()` → `presenter.present()`. crossterm raw mode for
  keyboard polling on the same thread (non-blocking reads).

## Open Questions

- Is the terminal a first-class play target or a novelty/debug view?
- Input abstraction — how window events and terminal keypresses unify into
  a single engine-level input model (post-MVP).
- Headless mode — can the window be skipped entirely for ASCII-only runs?
  (MVP already separates binaries, so partly answered.)
- Should both presenters run from a single binary later, or stay split?

## Decisions Log

- 2026-04-25: Off-screen-render + pluggable presenter architecture.
- 2026-04-25: Single unified pipeline for 2D and 3D, differentiated by camera
  projection and pipeline state, not separate pipelines.
- 2026-04-25: Use **viuer** as the terminal-rendering lib for MVP. Re-evaluate
  if per-frame performance becomes a problem.
- 2026-04-25: Skip `ratatui` for MVP. Terminal presenter is pure framebuffer
  (`viuer` + `crossterm`); revisit `ratatui-image` only if dev HUD chrome
  becomes essential.
- 2026-04-25: MVP shape locked — single library crate + two binaries
  (`window`, `terminal`); `Presenter` trait takes a `FrameCtx` referencing
  the shared offscreen `Rgba8UnormSrgb` texture; vertex = position + normal;
  Lambert shader; fixed orbit camera; no shared frame loop in MVP.
- 2026-04-25: MVP color = 24-bit truecolor via half-block (viuer auto-falls
  back per terminal capability); offscreen target ≈256×144 px.
