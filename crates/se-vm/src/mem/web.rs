//! The wasm32 `Mem` impl: a browser-owned `WebAssembly.Memory`.
//!
//! This is not a thin wrapper the way `NativeMem` is. Native's `data()` works
//! because wasmtime runs the guest in the *same* process as the host, so
//! `Memory::data` hands back a real pointer into memory the host already
//! shares. On wasm32 the arena instead lives in a `WebAssembly.Memory` JS
//! object: a separate `ArrayBuffer` outside this module's own linear memory,
//! reachable only by crossing the JS boundary. There is no pointer in this
//! module that could ever alias into that `ArrayBuffer` — `unsafe` buys
//! nothing there, because the two never share an address space to begin with
//! (contrast native, where the guest and the host *do* share one, which is
//! exactly what makes its unchecked pointer valid). A real zero-copy borrow
//! of the JS side is therefore not on the table; `data()` has to choose
//! between an owned copy and a cached one.
//!
//! This picks the cached fixed-capacity view (candidate (b) in
//! story-229/task-256's Target Invariant 2): `scratch` is sized once, at
//! construction, to `memory`'s byte length, and is never resized afterward —
//! only overwritten in place. That is sound only because this engine's arena
//! memory is created non-growable (`vm.rs`'s native path:
//! `MemoryType::new(pages, Some(pages))`, i.e. `min == max`); the web side
//! must mirror that when it builds the `WebAssembly.Memory` descriptor
//! (`initial == maximum == required_pages()`), which keeps the `ArrayBuffer`
//! byte length — and therefore `scratch`'s length — fixed for the memory's
//! whole lifetime. A buffer that only ever shrinks to zero capacity changes
//! (none here, since it is never resized) never moves, so a slice into it
//! stays valid for as long as `self` does. Candidate (a) (copy into a
//! `Vec` rebuilt fresh each call) would have been the honest fallback if
//! growth could happen here, but it cannot: trading an owned-copy-per-call
//! design for a cache that cannot itself be grown on a target where growth
//! never occurs is the one that matches how `Vm::bytes()`'s callers actually
//! use the result (`se-gpu::replay` holds the slice across a
//! `write_buffer` call, which an API that may reallocate out from under it
//! mid-use would not serve any better than an owned copy would).
//!
//! The remaining, weaker caveat — a `data()` call overwrites `scratch`'s
//! *contents* in place, so a slice from an earlier call observes the later
//! call's bytes once it returns — is the same class of risk wasmtime's own
//! `Memory::data` already carries on native (a `memory.grow` between two
//! live slices invalidates the earlier one); it is not a new risk this impl
//! introduces, only one inherited from the shape of a caching `&self`-based
//! API, and `World`/`Vm` do not yet call this impl at all (that is T-d).

use super::Mem;
use anyhow::Result;
use js_sys::Uint8Array;
use std::cell::UnsafeCell;

pub struct WebMem {
    pub memory: js_sys::WebAssembly::Memory,
    /// Fixed at construction to `memory`'s byte length; never resized after
    /// that — see the module doc for why that is what makes `data()` sound.
    scratch: UnsafeCell<Vec<u8>>,
}

impl WebMem {
    /// `memory` must be non-growable (`initial == maximum`), matching
    /// native's `MemoryType::new(pages, Some(pages))` — see the module doc.
    pub fn new(memory: js_sys::WebAssembly::Memory) -> Self {
        let len = Uint8Array::new(&memory.buffer()).length() as usize;
        Self {
            memory,
            scratch: UnsafeCell::new(vec![0u8; len]),
        }
    }
}

impl Mem for WebMem {
    fn read(&self, at: u32, len: u32) -> Result<Vec<u8>> {
        let view = Uint8Array::new(&self.memory.buffer());
        let mut v = vec![0u8; len as usize];
        view.subarray(at, at + len).copy_to(&mut v);
        Ok(v)
    }

    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<()> {
        let view = Uint8Array::new(&self.memory.buffer());
        view.set(&Uint8Array::from(bytes), at);
        Ok(())
    }

    fn data(&self) -> &[u8] {
        // SAFETY: `scratch`'s `Vec` is sized once in `new` and never
        // resized again (no `resize`/`push`/`shrink_to_fit` anywhere in this
        // impl), so its backing allocation never moves for the lifetime of
        // `self`; a `&[u8]` into it tied to `&self` is therefore valid even
        // though a later `data()` call overwrites its contents (see the
        // module doc's caveat). `copy_to` below requires the source and
        // destination lengths to match exactly, which holds because
        // `memory`'s `ArrayBuffer` is non-growable (constructor contract).
        let scratch = unsafe { &mut *self.scratch.get() };
        Uint8Array::new(&self.memory.buffer()).copy_to(scratch);
        scratch
    }
}
