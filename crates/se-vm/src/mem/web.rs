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
//! This picks a double-buffered cache (candidate (b) in story-229/task-256's
//! Target Invariant 2, refined by task-286's aliasing review): two
//! fixed-capacity `Vec<u8>`s, each sized once at construction to `memory`'s
//! byte length and never resized afterward — only overwritten in place.
//! `data()` alternates which buffer it writes into on each call, so the
//! `&mut` reborrow one call performs can never target the same allocation as
//! a `&[u8]` a *different* call has already handed out and a caller may
//! still be holding — the two allocations are entirely independent, so
//! nothing in this type ties their Stacked-Borrows provenance together. A
//! single shared buffer did not have that property: a second call's `&mut`
//! reborrow of the same allocation invalidated a first call's still-live
//! `&[u8]` under Rust's Stacked-Borrows aliasing model, which is undefined
//! behavior if the first slice is read again afterward. This is sound only
//! because this engine's arena memory is created non-growable (`vm.rs`'s
//! native path: `MemoryType::new(pages, Some(pages))`, i.e. `min == max`);
//! the web side must mirror that when it builds the `WebAssembly.Memory`
//! descriptor (`initial == maximum == required_pages()`), which keeps the
//! `ArrayBuffer` byte length — and therefore each buffer's length — fixed
//! for the memory's whole lifetime. A buffer that is never resized never
//! moves, so a slice into it stays valid for as long as `self` does.
//! Candidate (a) (copy into a `Vec` rebuilt fresh each call) would have been
//! the honest fallback if growth could happen here, but it cannot, and it
//! would also violate `Mem::data`'s contract that `se-gpu::replay`'s
//! `write_buffer` call relies on: a real borrow, not an owned copy.
//!
//! The residual caveat — three or more `data()` results held alive at once
//! would see the oldest alias the buffer a later call reuses — is far
//! narrower than the single-buffer design's hazard (any *two* overlapping
//! calls already triggered it), and matches how every current and
//! anticipated caller actually uses the result: at most one `data()` slice
//! alive at a time (`Vm::bytes()`/`Vm::snapshot()` in `vm/web.rs` each do
//! exactly one call per invocation, and `World`/`Vm` do not yet call this
//! impl at all — that is T-d).

use super::Mem;
use anyhow::{anyhow, Result};
use js_sys::Uint8Array;
use std::cell::{Cell, UnsafeCell};

pub struct WebMem {
    pub memory: js_sys::WebAssembly::Memory,
    /// Two buffers, each fixed at construction to `memory`'s byte length and
    /// never resized after that. `data()` alternates which one it writes
    /// into (tracked by `next`) — see the module doc for why that is what
    /// makes `data()` sound against two overlapping calls.
    scratch: [UnsafeCell<Vec<u8>>; 2],
    next: Cell<usize>,
}

impl WebMem {
    /// `memory` must be non-growable (`initial == maximum`), matching
    /// native's `MemoryType::new(pages, Some(pages))` — see the module doc.
    pub fn new(memory: js_sys::WebAssembly::Memory) -> Self {
        let len = Uint8Array::new(&memory.buffer()).length() as usize;
        Self {
            memory,
            scratch: [
                UnsafeCell::new(vec![0u8; len]),
                UnsafeCell::new(vec![0u8; len]),
            ],
            next: Cell::new(0),
        }
    }
}

impl Mem for WebMem {
    fn read(&self, at: u32, len: u32) -> Result<Vec<u8>> {
        let view = Uint8Array::new(&self.memory.buffer());
        let end = (at as u64)
            .checked_add(len as u64)
            .filter(|&end| end <= view.length() as u64)
            .ok_or_else(|| anyhow!("WebMem::read out of bounds: at={at} len={len}"))?;
        let mut v = vec![0u8; len as usize];
        view.subarray(at, end as u32).copy_to(&mut v);
        Ok(v)
    }

    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<()> {
        let view = Uint8Array::new(&self.memory.buffer());
        (at as u64)
            .checked_add(bytes.len() as u64)
            .filter(|&end| end <= view.length() as u64)
            .ok_or_else(|| anyhow!("WebMem::write out of bounds: at={at} len={}", bytes.len()))?;
        view.set(&Uint8Array::from(bytes), at);
        Ok(())
    }

    fn data(&self) -> &[u8] {
        // SAFETY: each call writes into the *other* buffer from the one the
        // previous call used, so the `&mut` reborrow below never targets the
        // same allocation as a `&[u8]` this method has already handed out
        // and that a caller may still be holding — see the module doc for
        // why a single shared buffer made that a Stacked-Borrows aliasing
        // hazard. Either buffer's `Vec` is sized once in `new` and never
        // resized again (no `resize`/`push`/`shrink_to_fit` anywhere in this
        // impl), so its backing allocation never moves for the lifetime of
        // `self`. `copy_to` below requires the source and destination
        // lengths to match exactly, which holds because `memory`'s
        // `ArrayBuffer` is non-growable (constructor contract).
        let idx = self.next.get();
        self.next.set(1 - idx);
        let scratch = unsafe { &mut *self.scratch[idx].get() };
        Uint8Array::new(&self.memory.buffer()).copy_to(scratch);
        scratch
    }
}
