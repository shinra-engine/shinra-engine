//! A seam between `World`/`Vm`'s byte-level arena access and whatever host
//! actually owns linear memory. `World::place/set_len/len_of/put_blob` need
//! only a bounded read, a bounded write, and the whole arena as a borrowed
//! slice — never wasmtime's `Memory`/`Store` themselves. Naming the trait
//! instead of the concrete pair is what lets a non-native host (a browser,
//! driving `WebAssembly.Memory` instead of wasmtime) implement the same
//! three operations later without `World`/`Vm` changing at all.

pub mod native;

use anyhow::Result;

pub trait Mem {
    fn read(&self, at: u32, len: u32) -> Result<Vec<u8>>;
    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<()>;
    /// The whole arena, borrowed. `se-gpu` hands a slice of this straight to
    /// `write_buffer`, so an implementation must return a real `&[u8]`, never
    /// an owned copy.
    fn data(&self) -> &[u8];
}
