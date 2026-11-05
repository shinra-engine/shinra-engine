//! The wasm32 bodies of `Vm`'s methods — see `vm.rs`'s module doc for why
//! these are a second `impl Vm` block rather than a second `Vm` type.
//!
//! Each method here does the same job as its native counterpart in `vm.rs`,
//! over `WebMem` instead of `(wasmtime::Memory, wasmtime::Store<()>)`: there
//! is no `wasmtime::Engine`/`Store` reachable from inside a wasm32 binary at
//! all (story-229's T1 finding), so these bodies go through `module::web`'s
//! `Loaded`/`Prepared` (no store to thread through) and `mem::web::WebMem`
//! (the `Mem` impl) instead.

use super::Vm;
use crate::mem::web::WebMem;
use crate::mem::Mem;
use crate::memory::{required_pages, Slot};
use crate::module::{Loaded, Prepared};
use crate::save;
use crate::world::{Meta, World};
use anyhow::{Context, Result};
use js_sys::wasm_bindgen::JsValue;
use js_sys::{Object, Reflect, WebAssembly};
use se_contract::read::Table;

/// A non-growable `WebAssembly.Memory`, matching native's
/// `MemoryType::new(pages, Some(pages))` (`initial == maximum`). `WebMem`'s
/// cached `data()` view is sound only under that contract — see
/// `mem/web.rs`'s module doc.
fn web_memory(pages: u32) -> Result<WebAssembly::Memory> {
    let descriptor = Object::new();
    Reflect::set(&descriptor, &JsValue::from_str("initial"), &JsValue::from_f64(pages as f64))
        .map_err(|e| anyhow::anyhow!("building the memory descriptor: {e:?}"))?;
    Reflect::set(&descriptor, &JsValue::from_str("maximum"), &JsValue::from_f64(pages as f64))
        .map_err(|e| anyhow::anyhow!("building the memory descriptor: {e:?}"))?;
    WebAssembly::Memory::new(&descriptor)
        .map_err(|e| anyhow::anyhow!("creating WebAssembly.Memory: {e:?}"))
}

impl Vm {
    /// Stand up a memory and place the world the contract describes.
    pub fn new(contract_wasm: &[u8], rows: u32) -> Result<Vm> {
        let table_bytes = se_contract::wasm::section(contract_wasm, se_contract::table::SECTION)
            .context("contract.wasm carries no se.contract section")?;
        let contract_id = save::fingerprint(table_bytes);
        let table = Table::new(table_bytes)?;
        let meta = Meta::read(&table)?;

        let pages = required_pages();
        let memory = web_memory(pages)?;
        let mut mem = WebMem::new(memory);
        let world = World::place(&meta, &mut mem, rows)?;

        Ok(Vm {
            mem,
            meta,
            world,
            game: None,
            process: None,
            render: None,
            stages: Vec::new(),
            staged: Vec::new(),
            rolled_back: None,
            contract_id,
            overruns: Vec::new(),
        })
    }

    /// Compile and check a module's bytes. There is no `wasmtime::Engine` to
    /// thread through on this target (T1's finding), unlike native's
    /// `prepare_module`.
    pub(crate) fn prepare_module(&self, slot: Slot, name: &str, bytes: Vec<u8>) -> Result<Prepared> {
        Loaded::prepare(slot, name, bytes, self.contract_id)
    }

    /// Instantiate a prepared module against this VM's shared memory.
    pub(crate) fn commit_module(&mut self, p: Prepared) -> Result<Loaded> {
        Loaded::commit(p, &self.mem.memory)
    }

    /// An empty slot is not an error: a game with no render module still
    /// ticks, which is what makes the modules independently replaceable.
    ///
    /// Unlike native, there is no fuel budget here: the Web API has no
    /// equivalent, so a runaway stage can only hang the tab until the
    /// browser itself intervenes (story-229/task-237's T2 design, an
    /// accepted behavior difference for this target).
    pub(crate) fn call(&mut self, slot: Slot, sym: &str, arg: Option<u32>) -> Result<u32> {
        let Some(m) = self.module(slot) else {
            return Ok(0);
        };
        match arg {
            Some(a) => m.func1(sym, a),
            None => m.func0(sym),
        }
    }

    /// Copy an asset's bytes into the blob region and publish its span.
    pub fn put_blob(&mut self, slot: u32, payload: &[u8]) -> Result<(u32, u32)> {
        self.world.put_blob(&self.meta, &mut self.mem, slot, payload)
    }

    /// Ask for a pool to hold `len` rows. Nothing spawns: a control var asks,
    /// and the host answers by moving one number.
    pub fn set_len(&mut self, pool: u32, len: u32) -> Result<()> {
        self.world.set_len(&self.meta, &mut self.mem, pool, len)
    }

    /// The current length of a pool, read back from the arena.
    pub(crate) fn read_pool_len(&mut self, pool: u32) -> Result<u32> {
        self.world.len_of(&self.meta, &self.mem, pool)
    }

    /// The whole arena, byte for byte. A save file is this and nothing else —
    /// there is no entity table to serialise, because there are no entities.
    pub fn snapshot(&self) -> Vec<u8> {
        let end = self.world.blob_top as usize;
        self.mem.data()[self.meta.base as usize..end].to_vec()
    }

    pub fn read(&self, at: u32, len: u32) -> Result<Vec<u8>> {
        self.mem.read(at, len)
    }

    pub fn write(&mut self, at: u32, bytes: &[u8]) -> Result<()> {
        self.mem.write(at, bytes)
    }

    /// The whole of linear memory, for handing a column straight to the GPU.
    pub fn bytes(&self) -> &[u8] {
        self.mem.data()
    }
}
