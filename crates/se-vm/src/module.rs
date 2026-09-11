//! Loading a module, and refusing one that would break the memory model.
//!
//! Two checks matter more than the loading.
//!
//! A module must import **only** the shared memory. An import that can call
//! back into another module would put two modules on the one shared stack at
//! once, and `memory.rs` explains why that region is only safe while exactly
//! one module is ever on it. So the rule the design states — *game.wasm 只是
//! 更新 contract.wit 裡面的control var sets* — is enforced here rather than
//! trusted.
//!
//! And a module must export no memory of its own. One that does was built
//! without `--import-memory`, and everything it writes would land somewhere
//! the host never reads. That failure is silent, so it is caught at load.

use crate::memory::Slot;
use anyhow::{bail, Context, Result};
use wasmtime::{Engine, Instance, Linker, Memory, Module as WasmModule, Store, TypedFunc};

pub struct Loaded {
    pub name: String,
    pub slot: Slot,
    pub instance: Instance,
    /// The file, kept so its custom sections stay readable after load.
    pub bytes: Vec<u8>,
}

/// Everything a module is allowed to ask the host for.
const MEMORY_IMPORT: (&str, &str) = ("env", "memory");

fn check_shape(name: &str, m: &WasmModule) -> Result<()> {
    let mut imports = Vec::new();
    for i in m.imports() {
        if (i.module(), i.name()) == MEMORY_IMPORT && i.ty().memory().is_some() {
            continue;
        }
        imports.push(format!("{}::{}", i.module(), i.name()));
    }
    if !imports.is_empty() {
        bail!(
            "`{name}` imports {imports:?}. A module may import only `env::memory`: \
             an import that re-enters another module would put two of them on the \
             one shared stack at once."
        )
    }
    if !m.imports().any(|i| i.ty().memory().is_some()) {
        bail!(
            "`{name}` imports no memory — it was built without `--import-memory`, so \
             nothing it writes would reach the world"
        )
    }
    if m.exports().any(|e| e.ty().memory().is_some()) {
        bail!("`{name}` exports its own memory; it is not sharing the host's")
    }
    Ok(())
}

impl Loaded {
    pub fn open(
        engine: &Engine,
        store: &mut Store<()>,
        memory: Memory,
        slot: Slot,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<Loaded> {
        let m = WasmModule::from_binary(engine, &bytes)
            .with_context(|| format!("`{name}` is not a wasm module"))?;
        check_shape(name, &m)?;

        let mut linker = Linker::new(engine);
        linker.define(&mut *store, MEMORY_IMPORT.0, MEMORY_IMPORT.1, memory)?;
        let instance = linker
            .instantiate(&mut *store, &m)
            .with_context(|| format!("instantiating `{name}` into the {} slot", slot.name()))?;

        Ok(Loaded { name: name.to_string(), slot, instance, bytes })
    }

    /// A section the module carries as data — `se.wgsl` for a render module,
    /// `se.asset` for content. Read from the file, so no instantiation is
    /// needed to get at it.
    pub fn section(&self, name: &str) -> Result<&[u8]> {
        se_contract::wasm::section(&self.bytes, name)
            .with_context(|| format!("in `{}`", self.name))
    }

    pub fn func0(&self, store: &mut Store<()>, sym: &str) -> Result<TypedFunc<(), u32>> {
        self.instance
            .get_typed_func::<(), u32>(&mut *store, sym)
            .with_context(|| format!("`{}` exports no {sym}()", self.name))
    }

    pub fn func1(&self, store: &mut Store<()>, sym: &str) -> Result<TypedFunc<u32, u32>> {
        self.instance
            .get_typed_func::<u32, u32>(&mut *store, sym)
            .with_context(|| format!("`{}` exports no {sym}(u32)", self.name))
    }

    pub fn has(&self, store: &mut Store<()>, sym: &str) -> bool {
        self.instance.get_func(&mut *store, sym).is_some()
    }

    /// Every export whose name begins with `prefix`.
    ///
    /// This is how a stage is found rather than listed: `contract.wit` closes
    /// the set of *types*, and a process module may export any number of
    /// stages over them, including ones written long after this host was
    /// built.
    pub fn exports_with(&self, store: &mut Store<()>, prefix: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .instance
            .exports(&mut *store)
            .filter_map(|e| {
                let n = e.name();
                n.starts_with(prefix).then(|| n.to_string())
            })
            .collect();
        v.sort();
        v
    }
}
