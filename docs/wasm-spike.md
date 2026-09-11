# wasm spike — measured, 2026-09-06

> **Closed.** The gap this document measured has since been fixed: the sinks
> are inverted and every module of game1 now builds for wasm32. What follows is
> the measurement that decided the shape, kept because the reasoning is the
> record. The section at the foot says what actually happened, including the
> one thing the measurement did not predict.

The question was how big the gap is between the `.so` backend and a wasm one.
The answer is smaller than expected in one direction and completely undone in
the other, and it is worth writing down which is which before designing for it.

## What was done

Every module of `game1` was compiled for `wasm32-unknown-unknown` using the
same `rustc` invocation `se-cli/src/build.rs` already emits, plus `--target`.

**No source file was changed. No `#[cfg]` was added. Nothing was ported.**

```
asset_bunny.wasm       exports: memory se_abi_version se_register_assets
asset_quad.wasm        exports: memory se_abi_version se_register_assets
bundle.wasm            exports: memory se_abi_version se_register_buffers se_register_layouts
game_game1.wasm        exports: memory se_abi_version se_register_control se_register_layouts
process_breathe.wasm   exports: memory se_abi_version se_register_layouts se_register_stages
process_orbit.wasm     exports: memory se_abi_version se_register_layouts se_register_stages
process_spin.wasm      exports: memory se_abi_version se_register_layouts se_register_stages
render_ghost.wasm      exports: memory se_abi_version se_register_graph
render_solid.wasm      exports: memory se_abi_version se_register_graph
```

All nine import **nothing**.

## What that means

`#[no_mangle] pub extern "C" fn` is already a wasm export. The six entry points
in `se/src/entry.rs` need no wasm arm — they are the same declaration on both
backends. So is `data.rs`: it compiled to an rlib for wasm and every module
linked against it, which means "obeys the bundle contract" stays a
compile-time fact on the wasm side too.

The exported `memory` is the other half of the good news. `Space::records`
hands out `bytes: &[u8]` rather than a pointer precisely so it can mean a
range of a wasm linear memory, and here that memory is right there as an
export.

## The gap, exactly

Compilation is free. The **call protocol** is the whole of the gap, and the
zero-import line above is how you can see it:

```rust
pub struct LayoutSink {
    pub ctx: *mut core::ffi::c_void,
    pub push: unsafe extern "C" fn(*mut c_void, *const Layout),
}
```

The host passes a struct carrying a host function pointer and the module calls
it. Inside wasm that pointer is a 32-bit number naming nothing — the module
has no way to call it and, as measured, does not import anything that could
stand in for it. `se_register_layouts` therefore *compiles* on wasm and would
be wrong at run time. `Ctl` is the same shape, with more arms.

So the gap is not "port the modules". It is one decision about how a module
hands data back.

## The direction that closes it

Invert the sinks: instead of the host handing in a callback, the module
serialises its registrations into its own memory and returns `(offset, len)`.

- `.so` — the host reads that pointer directly, as it does today.
- `.wasm` — the host reads the same bytes out of the exported `memory` at that
  offset.

One source, no `#[cfg]`, and the modules keep the zero-import purity they were
just measured to have. `Requests` in `se-host/src/ctl.rs` is already this
shape for slots, swipe, quit, save and load — the tick collects intentions and
the host applies them at the frame boundary. Extending that to the rest of
`Ctl` is finishing a pattern the design already committed to, not adopting a
new one.

This is why the wasm question makes the design cleaner rather than dirtier:
the thing wasm cannot express is exactly the thing that was already the
sharpest edge in the native ABI.

## Reproducing

```sh
rustup target add wasm32-unknown-unknown
cargo build -p se --target wasm32-unknown-unknown
# proc macros are host artifacts, so the wasm build needs BOTH dependency paths
rustc --edition 2021 --target wasm32-unknown-unknown \
  -L dependency=target/wasm32-unknown-unknown/debug/deps \
  -L dependency=target/debug/deps \
  --extern se=<wasm libse.rlib> --extern data=<wasm libdata.rlib> \
  --crate-type cdylib --crate-name spin \
  target/shinra/game1/.gen/process_spin_root.rs -o spin.wasm
```

The two `-L` paths are the one real build-system change: `se-macro` is a
proc-macro crate and is always built for the host, never for the target.

---

## What happened when it was done

The inversion went in as described: six entry points returning `*const
Registered`, the host reading back instead of being called per item. Three
games render byte-identical (252 / 73 / 1951), nineteen test targets green.

One thing this document did not predict, and only a real wasm build found:

**`#[link_section]` cannot hold a pointer on wasm.** Ticket 26's
`#[se::stage]` collects stages by putting a `StageEntry` — a pointer to a
`StageSpec` — into an `se_stages` section, and `se_register_stages` walks it.
That is sound on a `.so` and impossible on wasm, where a custom section must
be a plain run of bytes and a pointer is a relocation. rustc says so directly:

```
error: statics with a custom `#[link_section]` must be a simple list of bytes
on the wasm target with no extra levels of indirection such as references
```

So the collection mechanism, not the data shape, is what needed the `#[cfg]`.
On wasm each `#[se::stage]` exports itself as `se_stage__<name>` and the
loader enumerates exports where the native one walks a section:

```
process_spin.wasm   imports=0  [memory, se_abi_version, se_register_layouts,
                                se_register_stages, se_stage__spin]
```

The divergence is confined to `se`. `#[se::stage]` is the same attribute on
both backends, and a game never learns which one it got — which is the whole
requirement: *example games 兩條路都能走*.

All nine modules still import nothing.

## Still to do

The wasm *host* does not exist yet. Everything above is the module side: the
contract is now expressible on both backends and the modules build. Reading a
`Registered` out of a wasm module's linear memory, walking the nested `Str`
and `Slice` offsets, and driving a graph on WebGPU are the next piece of work,
and none of it needs a module to change again.
