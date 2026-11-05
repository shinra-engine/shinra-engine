//! The native `Mem` impl: a thin wrapper over the `(Memory, &mut Store<()>)`
//! pair `Vm` already holds, built fresh at each call site. `wasmtime::Memory`
//! is `Copy`, so this costs nothing and reads/writes behave byte-for-byte
//! like calling `wasmtime::Memory::read/write/data` directly — this file
//! changes no behavior, only the shape of the call.

use super::Mem;
use anyhow::Result;
use wasmtime::{Memory, Store};

pub struct NativeMem<'a> {
    pub memory: Memory,
    pub store: &'a mut Store<()>,
}

impl Mem for NativeMem<'_> {
    fn read(&self, at: u32, len: u32) -> Result<Vec<u8>> {
        let mut v = vec![0u8; len as usize];
        self.memory.read(&*self.store, at as usize, &mut v)?;
        Ok(v)
    }

    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<()> {
        self.memory.write(&mut *self.store, at as usize, bytes)?;
        Ok(())
    }

    fn data(&self) -> &[u8] {
        self.memory.data(&*self.store)
    }
}
