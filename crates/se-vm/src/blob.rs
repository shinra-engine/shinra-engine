//! The blob-slot helpers: publish bytes into the blob region, then read a
//! slot's span back or copy it into a record field.
//!
//! Split out of `vm.rs` to keep that file under the project's 400-line
//! convention; these are `impl Vm` blocks like any other, just in their own
//! module because none of them is `cfg`-gated per target — each goes through
//! `Vm::put_blob`/`read`/`write`, whose bodies differ by target, not these.

use crate::vm::Vm;
use crate::world::SPAN_BYTES;
use anyhow::{bail, Result};

impl Vm {
    pub fn put_str(&mut self, slot: u32, s: &str) -> Result<(u32, u32)> {
        self.put_blob(slot, s.as_bytes())
    }

    pub fn put_u32s(&mut self, slot: u32, v: &[u32]) -> Result<(u32, u32)> {
        let b: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.put_blob(slot, &b)
    }

    pub fn put_f32s(&mut self, slot: u32, v: &[f32]) -> Result<(u32, u32)> {
        let b: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.put_blob(slot, &b)
    }

    /// The span of a blob slot, as published in the arena.
    fn span(&self, slot: u32) -> Result<[u8; 8]> {
        if slot >= self.meta.blob_slots {
            bail!("blob slot {slot} is past the {} the contract declares", self.meta.blob_slots)
        }
        let v = self.read(self.meta.blobs_at + slot * SPAN_BYTES, SPAN_BYTES)?;
        Ok(v.try_into().expect("read returns the length asked"))
    }

    /// The bytes a blob slot's span names.
    pub fn blob(&self, slot: u32) -> Result<Vec<u8>> {
        let s = self.span(slot)?;
        let off = u32::from_le_bytes(s[..4].try_into().unwrap());
        let len = u32::from_le_bytes(s[4..].try_into().unwrap());
        self.read(off, len)
    }

    /// Copy a slot's span into a record field at arena address `at`.
    pub fn set_span_field(&mut self, at: u32, slot: u32) -> Result<()> {
        let s = self.span(slot)?;
        self.write(at, &s)
    }
}
