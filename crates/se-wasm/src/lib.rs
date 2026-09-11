//! The wasm backend: the same contract, read out of a linear memory.
//!
//! A module is a module. It exports the same six entry points with the same
//! names, takes nothing, and returns where its registrations are — the work
//! `se-host`'s `Module` does by dereferencing a pointer, this does by reading
//! bytes at an offset. That symmetry is the whole point of inverting the
//! sinks: nothing in a game, and nothing in `se`'s public surface, knows which
//! backend it was built for.
//!
//! What does differ, and cannot be hidden, is that wasm32 lays its structs out
//! differently — four-byte pointers — so the bytes are parsed rather than
//! reinterpreted. That lives in `decode`, and `tests/parity.rs` holds it
//! honest by requiring the same source built both ways to produce the same
//! definitions.

pub mod call;
pub mod decode;
pub mod graph;
pub mod mem;

use anyhow::{bail, Context, Result};
use mem::{Mem, Registered};
use se_host::registry::{BufferDef, LayoutDef};
use std::path::Path;
use wasmtime::{Engine, Instance, Linker, Memory, Module as WModule, Store};

/// One loaded `.wasm`, with its memory and its exports.
pub struct WasmModule {
    pub name: String,
    store: Store<()>,
    instance: Instance,
    memory: Memory,
}

impl WasmModule {
    pub fn open(engine: &Engine, path: &Path) -> Result<WasmModule> {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let module = WModule::from_file(engine, path)
            .with_context(|| format!("reading {}", path.display()))?;

        // A module imports nothing — that is checked, not assumed. An import
        // would mean it expects something of the host that the `.so` backend
        // never provides, and the two backends are meant not to diverge.
        if module.imports().len() != 0 {
            let names: Vec<String> =
                module.imports().map(|i| format!("{}.{}", i.module(), i.name())).collect();
            bail!("`{name}` imports {} — a module must be self-contained: {names:?}", names.len());
        }

        let mut store = Store::new(engine, ());
        let instance = Linker::new(engine)
            .instantiate(&mut store, &module)
            .with_context(|| format!("instantiating {name}"))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .with_context(|| format!("`{name}` exports no memory"))?;

        let mut m = WasmModule { name, store, instance, memory };
        m.check_abi()?;
        Ok(m)
    }

    fn check_abi(&mut self) -> Result<()> {
        // `se_abi_version` returns a struct by value, which the C ABI on wasm
        // returns through an out-pointer the caller supplies. Rather than hand
        // the module memory it did not allocate, the version is read the same
        // way everything else is once a real host needs it; for now its
        // presence is the check.
        if !self.has("se_abi_version") {
            bail!("`{}` exports no se_abi_version", self.name);
        }
        Ok(())
    }

    pub fn has(&mut self, sym: &str) -> bool {
        self.instance.get_func(&mut self.store, sym).is_some()
    }

    /// Call one `se_register_*` and read back the descriptor it points at.
    fn registered(&mut self, sym: &str) -> Result<Registered> {
        let f = self
            .instance
            .get_typed_func::<(), i32>(&mut self.store, sym)
            .with_context(|| format!("`{}` exports no {sym}", self.name))?;
        let at = f.call(&mut self.store, ())? as u32;
        if at == 0 {
            return Ok(Registered { ptr: 0, len: 0, stride: 0 });
        }
        let bytes = self.memory.data(&self.store);
        Registered::read(&Mem { bytes }, at)
    }

    pub fn layouts(&mut self) -> Result<Vec<LayoutDef>> {
        let r = self.registered("se_register_layouts")?;
        let bytes = self.memory.data(&self.store);
        decode::layouts(&Mem { bytes }, &r)
    }

    pub fn buffers(&mut self) -> Result<Vec<BufferDef>> {
        let r = self.registered("se_register_buffers")?;
        let bytes = self.memory.data(&self.store);
        decode::buffers(&Mem { bytes }, &r)
    }

    pub fn graphs(&mut self) -> Result<Vec<se_host::registry::GraphDef>> {
        let r = self.registered("se_register_graph")?;
        let bytes = self.memory.data(&self.store);
        graph::graphs(&Mem { bytes }, &r)
    }

    /// Every stage this module carries, described but not yet callable.
    ///
    /// `se_stage__<name>` hands back the spec's address; the spec's own name
    /// is read from there rather than from the export, so the two cannot drift
    /// apart. Running one is a separate problem: `StageSpec` ends in a
    /// function pointer, and a stage's data has to reach the module's own
    /// memory before it can be called at all.
    pub fn stages(&mut self) -> Result<Vec<graph::StageInfo>> {
        let mut out = Vec::new();
        for export in self.stage_names() {
            let sym = format!("se_stage__{export}");
            let f = self
                .instance
                .get_typed_func::<(), i32>(&mut self.store, &sym)
                .with_context(|| format!("`{}` exports no {sym}", self.name))?;
            let at = f.call(&mut self.store, ())? as u32;
            let bytes = self.memory.data(&self.store);
            out.push(graph::stage_at(&Mem { bytes }, at)?);
        }
        Ok(out)
    }

    /// Run one stage over data the host holds, copying it in and back out.
    ///
    /// Returns the columns the stage's signature says it writes, in parameter
    /// order — a `&Transform` parameter yields nothing, because it cannot have
    /// changed anything.
    pub fn run_stage(&mut self, stage: &str, c: &call::Call) -> Result<Vec<(usize, Vec<u8>)>> {
        let plan = c.plan()?;

        let alloc = self
            .instance
            .get_typed_func::<u32, i32>(&mut self.store, "se_alloc")
            .context("module exports no se_alloc — it carries no stage")?;
        let base = alloc.call(&mut self.store, plan.total)? as u32;
        if base == 0 {
            bail!("`{}` could not allocate {} bytes for a stage call", self.name, plan.total);
        }

        let mut block = vec![0u8; plan.total as usize];
        c.encode(&plan, base, &mut block)?;
        self.memory.write(&mut self.store, base as usize, &block)?;

        let sym = format!("se_stage_run__{stage}");
        let run = self
            .instance
            .get_typed_func::<i32, ()>(&mut self.store, &sym)
            .with_context(|| format!("`{}` exports no {sym}", self.name))?;
        run.call(&mut self.store, (base + plan.call_at) as i32)?;

        let bytes = self.memory.data(&self.store);
        let m = Mem { bytes };
        let mut out = Vec::new();
        for i in c.written() {
            out.push((i, call::read_back(&m, &plan, i, base)?.to_vec()));
        }

        // Freed rather than leaked: a stage runs every frame, and a host that
        // never gave the memory back would grow one allocation per frame.
        if let Ok(free) = self.instance.get_typed_func::<(u32, u32), ()>(&mut self.store, "se_free")
        {
            free.call(&mut self.store, (base, plan.total))?;
        }
        Ok(out)
    }

    /// The stages this module exports, by the name each one exports itself
    /// under.
    ///
    /// wasm cannot hold the link section the `.so` backend collects stages in,
    /// so `#[se::stage]` exports `se_stage__<name>` there and this enumerates
    /// them. Sorted, because export order is the linker's business.
    pub fn stage_names(&mut self) -> Vec<String> {
        let mut out: Vec<String> = self
            .instance
            .exports(&mut self.store)
            .filter_map(|e| e.name().strip_prefix("se_stage__").map(str::to_string))
            .collect();
        out.sort();
        out
    }
}

/// One engine shared by every module in a bundle, so they compile once.
pub fn engine() -> Result<Engine> {
    Ok(Engine::default())
}
