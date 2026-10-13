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
use wasmtime::{
    Engine, ExternType, Instance, Linker, Memory, Module as WasmModule, Store, Trap, TypedFunc,
    ValType,
};

/// What one entry into wasm may spend before it is cut off. A frame crosses
/// into a module a handful of times and each stage is a tight loop, so this is
/// far above anything healthy and still trips in well under a second.
pub const DEFAULT_FUEL: u64 = 200_000_000;

/// Give the store a fresh budget, run `f`, and say so if the budget was what
/// ended it. `what` names the entry: a module's symbol, or its instantiation.
pub fn with_budget<T>(
    store: &mut Store<()>,
    what: &str,
    f: impl FnOnce(&mut Store<()>) -> Result<T>,
) -> Result<T> {
    store.set_fuel(DEFAULT_FUEL)?;
    f(store).map_err(|e| {
        if e.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel) {
            e.context(format!("{what} ran out of its budget of {DEFAULT_FUEL} fuel"))
        } else {
            e
        }
    })
}

pub struct Loaded {
    pub name: String,
    pub slot: Slot,
    pub instance: Instance,
    /// The file, kept so its custom sections stay readable after load.
    pub bytes: Vec<u8>,
}

/// A module compiled and checked, but not instantiated: nothing it carries
/// has touched the shared memory yet, so refusing it costs nothing.
pub struct Prepared {
    pub name: String,
    pub slot: Slot,
    module: WasmModule,
    bytes: Vec<u8>,
}

impl Prepared {
    pub fn section(&self, name: &str) -> Result<&[u8]> {
        se_contract::wasm::section(&self.bytes, name)
            .with_context(|| format!("in `{}`", self.name))
    }

    pub fn has_export(&self, sym: &str) -> bool {
        self.module.get_export(sym).is_some()
    }
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

/// The entry point a slot calls every frame, as (symbol, i32 params,
/// how the refusal spells it). A process module's stages are checked against
/// its own declaration instead.
fn entry(slot: Slot) -> Option<(&'static str, usize, &'static str)> {
    match slot {
        Slot::Game => Some(("tick", 1, "tick(u32)")),
        Slot::Render => Some(("record", 0, "record()")),
        Slot::Process => None,
    }
}

/// Checked on the compiled module, before any instance exists: a game with no
/// `tick` would otherwise replace the running one and fail every frame.
fn check_entry(name: &str, slot: Slot, m: &WasmModule) -> Result<()> {
    let Some((sym, params, shown)) = entry(slot) else { return Ok(()) };
    let ok = match m.get_export(sym) {
        Some(ExternType::Func(f)) => {
            let i32s = |v: Vec<ValType>, n: usize| {
                v.len() == n && v.iter().all(|t| matches!(t, ValType::I32))
            };
            i32s(f.params().collect(), params) && i32s(f.results().collect(), 1)
        }
        _ => false,
    };
    if !ok {
        bail!("`{name}` exports no {shown} -> u32; the {} slot calls it every frame", slot.name())
    }
    Ok(())
}

/// A module is only ever meaningful against the contract it was built from:
/// every offset in it came from that table. The id rides in a section that
/// `shinra build` writes, so it is read from the file, before anything runs.
pub fn check_contract_id(name: &str, bytes: &[u8], want: u64) -> Result<()> {
    let Ok(found) = se_contract::wasm::section(bytes, se_contract::table::CONTRACT_ID_SECTION)
    else {
        bail!("`{name}` carries no contract id; it was not built by `shinra build` against this contract")
    };
    if found != want.to_le_bytes() {
        let got = <[u8; 8]>::try_from(found).map(u64::from_le_bytes);
        let shown = got.map_or_else(|_| "malformed".to_string(), |g| format!("{g:016x}"));
        bail!("`{name}` was built against contract {shown}; this world is {want:016x} — run `shinra build`")
    }
    Ok(())
}

impl Loaded {
    /// Compile and check. No store and no memory are in reach, so a refusal
    /// here cannot have changed anything.
    pub fn prepare(
        engine: &Engine,
        slot: Slot,
        name: &str,
        bytes: Vec<u8>,
        contract_id: u64,
    ) -> Result<Prepared> {
        let module = WasmModule::from_binary(engine, &bytes)
            .with_context(|| format!("`{name}` is not a wasm module"))?;
        check_shape(name, &module)?;
        check_entry(name, slot, &module)?;
        check_contract_id(name, &bytes, contract_id)?;
        Ok(Prepared { name: name.to_string(), slot, module, bytes })
    }

    /// Instantiate: from here the module's data segments are in the memory.
    pub fn commit(p: Prepared, store: &mut Store<()>, memory: Memory) -> Result<Loaded> {
        let engine = p.module.engine().clone();
        let mut linker = Linker::new(&engine);
        linker.define(&mut *store, MEMORY_IMPORT.0, MEMORY_IMPORT.1, memory)?;
        // Instantiation runs a start function, so it is the second way in.
        let instance = with_budget(store, &format!("`{}` instantiation", p.name), |s| {
            linker.instantiate(&mut *s, &p.module).map_err(Into::into)
        })
        .with_context(|| format!("instantiating `{}` into the {} slot", p.name, p.slot.name()))?;
        Ok(Loaded { name: p.name, slot: p.slot, instance, bytes: p.bytes })
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
