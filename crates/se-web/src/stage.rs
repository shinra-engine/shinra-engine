//! design.md's middle station on this route: control decides, **stages
//! update**, the graph draws.
//!
//! A stage is discovered, not listed. `#[se::stage]` cannot file a pointer in
//! a custom section on wasm — a custom section there is a plain run of bytes
//! and a pointer to the spec is a relocation — so each stage exports itself
//! instead, under `se_stage__<name>`, and the loader enumerates exports where
//! the native one walks a link section. Adding a stage is still adding a file.
//!
//! Two exports per stage, and both are needed:
//!
//!   * `se_stage__<name>` answers with the `StageSpec` — the same spec the
//!     `.so` route reads, built by the same attribute from the same signature,
//!     so the parameter list *is* the query on both routes.
//!   * `se_stage_run__<name>` is the batched loop. The spec's `run` field is a
//!     table index in the module, which means nothing here, so the call goes
//!     by name; the per-entity loop still runs inside the module and the
//!     boundary is crossed once per stage.
//!
//! What is route-specific is where the columns are. Natively a stage is handed
//! raw pointers into the host's own columns. Here the store is in `se-web`'s
//! memory and the stage runs in the module's, so a column is copied in, the
//! stage runs over the copy, and the columns its signature declared `&mut` are
//! copied back — the bargain `se`'s `se_alloc` exists for, and the one
//! design.md measured at ~6 µs a frame for a thousand entities.
//!
//! `resolve` below is `se_host::schedule`'s, written a second time because
//! that function is private to it and `se-host` is not this ticket's to edit.
//! The declarations it works from — order, kind, access, size, hash — all come
//! from the module's own `StageSpec`, so the two routes cannot disagree about
//! *what* a stage wants; only about how the rows are gathered, which is the
//! same intersection either way.

use crate::module::{js_err, ModuleMem, WebModule};
use anyhow::{anyhow, bail, Context, Result};
use js_sys::{Function, Object};
use se_abi::{Access, Input, Param, ParamKind, Slice, StageCall, StageSpec, Str};
use se_host::registry::{ParamDef, StageDef};
use se_host::space::{ColumnRef, Store};
use std::mem::size_of;
use wasm_bindgen::JsValue;

/// What `#[se::stage]` names its two exports. The prefix is the contract: a
/// module is asked for nothing else and the engine knows no stage by name.
const SPEC: &str = "se_stage__";
const RUN: &str = "se_stage_run__";

/// `StageSpec` as it sits in a module's memory.
///
/// `run` is read as the integer it is, exactly as `module.rs` reads a
/// control's `start`/`tick`/`stop`: it is an index into *that* module's table
/// and this host must never be tempted to call it.
#[repr(C)]
#[derive(Clone, Copy)]
struct RawStage {
    name: Str,
    params: Slice<Param>,
    run: u32,
}

const _: () = assert!(size_of::<RawStage>() == size_of::<StageSpec>());

/// One stage, read out of the module that exported it.
pub struct Stage {
    pub name: String,
    params: Vec<ParamDef>,
    /// `se_stage_run__<name>`, held so the per-frame call is a plain
    /// invocation rather than another export lookup.
    run: Function,
}

/// `StageDef` carries the native route's call — a pointer the host
/// dereferences. Here the call is a named export, so the field is filled with
/// something that says so if anything ever reaches it. Only
/// `schedule::validate` is given these, and validation never runs a stage.
unsafe extern "C" fn by_name_only(_: *const StageCall) {
    unreachable!("a wasm stage is called through its `se_stage_run__` export")
}

impl Stage {
    fn def(&self) -> StageDef {
        StageDef { name: self.name.clone(), params: self.params.clone(), run: by_name_only }
    }
}

/// One module's stages, and the memory they run in.
struct Module {
    name: String,
    mem: ModuleMem,
    stages: Vec<Stage>,
}

/// Every stage a game's modules exported, in the order they run.
pub struct Stages {
    mods: Vec<Module>,
}

/// Read the stages out of every module handed over.
///
/// Modules that export none — the bundle, the graph, the theme, the control —
/// simply contribute none, so nothing has to say which of them is a `process`
/// module.
pub fn collect(names: &[String], exports: &[Object]) -> Result<Stages> {
    let mut mods = Vec::new();
    for (name, ex) in names.iter().zip(exports) {
        let m = WebModule::new(name.clone(), ex.clone());
        let stages = read(&m).with_context(|| format!("module `{name}`"))?;
        if stages.is_empty() {
            continue;
        }
        mods.push(Module { mem: m.mem()?, name: m.name.clone(), stages });
    }
    // Native takes `process/*.so` in sorted path order (`se_host`'s
    // `modules_in`). This is that order, reached from the loader's other end:
    // the module names are the file stems.
    mods.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Stages { mods })
}

fn read(m: &WebModule) -> Result<Vec<Stage>> {
    let syms: Vec<String> =
        m.export_names().into_iter().filter(|s| s.starts_with(SPEC)).collect();
    if syms.is_empty() {
        return Ok(Vec::new());
    }
    // Every spec call first, then the snapshot — `WebModule::read`'s rule, for
    // the same reason: a module may build what it answers with on demand.
    let mut ats = Vec::with_capacity(syms.len());
    for s in &syms {
        let v = m.export(s)?.call0(&JsValue::NULL).map_err(js_err)?;
        ats.push(v.as_f64().ok_or_else(|| anyhow!("{s} did not answer with an address"))? as u32);
    }

    let r = se_host::reloc::Reloc::new(m.memory()?);
    let mut out = Vec::with_capacity(syms.len());
    for at in ats {
        // SAFETY: `at` is what the module answered with, and `r` holds its
        // memory as it stood after that call.
        let (name, params) = unsafe {
            let raw: &RawStage = r.get(at).context("a stage spec is outside the module")?;
            let name = r.str(&raw.name).context("a stage name is outside the module")?.to_string();
            let ps = r.slice(&raw.params).context("a stage's params are outside the module")?;
            let params = ps
                .iter()
                .map(|p| {
                    Ok(ParamDef {
                        name: r
                            .str(&p.name)
                            .context("a param name is outside the module")?
                            .to_string(),
                        kind: p.kind,
                        access: p.access,
                        size: p.size,
                        hash: p.hash,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            (name, params)
        };
        let run = m.export(&format!("{RUN}{name}"))?;
        out.push(Stage { name, params, run });
    }
    Ok(out)
}

impl Stages {
    fn iter(&self) -> impl Iterator<Item = &Stage> {
        self.mods.iter().flat_map(|m| m.stages.iter())
    }

    /// Refuse a stage that does not fit the bundle contract, before it ever
    /// runs — `Session::stand_up`'s check, and literally the same function.
    pub fn validate(&self, store: &dyn Store) -> Result<()> {
        for s in self.iter() {
            se_host::schedule::validate(store, &s.def())
                .with_context(|| format!("stage `{}` does not fit the bundle contract", s.name))?;
        }
        Ok(())
    }

    /// Every stage, in order, over the whole world.
    pub fn run(&self, store: &mut dyn Store, input: &Input, dt: f32) -> Result<()> {
        for m in &self.mods {
            for s in &m.stages {
                run_one(&m.mem, s, store, input, dt)
                    .with_context(|| format!("stage `{}` in `{}`", s.name, m.name))?;
            }
        }
        Ok(())
    }

    /// What ran, for the page's report.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for m in &self.mods {
            for s in &m.stages {
                // The signature as declared, because the signature is the
                // query: what a stage can reach is exactly this list.
                let q: Vec<String> = s
                    .params
                    .iter()
                    .map(|p| match (p.kind, p.access) {
                        (ParamKind::Dt, _) => p.name.clone(),
                        (_, Access::Write) => format!("&mut {}", p.name),
                        (_, Access::Read) => format!("&{}", p.name),
                    })
                    .collect();
                out.push_str(&format!("    stage {} ({}) from {}\n", s.name, q.join(", "), m.name));
            }
        }
        out
    }
}

/// Scratch in a module's memory, freed however the call leaves.
struct Lease<'a> {
    mem: &'a ModuleMem,
    blocks: Vec<(u32, u32)>,
}

impl Lease<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<u32> {
        // `se_alloc` answers null for zero, and an empty resource list is a
        // real case, so a zero-length block still asks for a byte.
        let n = (bytes.len() as u32).max(1);
        let at = self.mem.alloc(n);
        if at == 0 || !self.mem.write(at, bytes) {
            self.mem.free(at, n);
            bail!("the stage's module would not take {n} bytes");
        }
        self.blocks.push((at, n));
        Ok(at)
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        for (at, n) in self.blocks.drain(..) {
            self.mem.free(at, n);
        }
    }
}

/// A column lent to the module for one call.
struct Lent {
    /// `Some` for a `&mut` parameter: the one kind of column that comes back.
    back: Option<String>,
    at: u32,
    len: u32,
}

fn run_one(
    mem: &ModuleMem,
    s: &Stage,
    store: &mut dyn Store,
    input: &Input,
    dt: f32,
) -> Result<()> {
    let comps: Vec<&ParamDef> =
        s.params.iter().filter(|p| p.kind == ParamKind::Component).collect();
    let (rows, n_rows) = resolve(store, &comps);
    if n_rows == 0 {
        return Ok(());
    }

    let mut lease = Lease { mem, blocks: Vec::new() };
    let mut lent = Vec::with_capacity(comps.len());
    let mut cols = Vec::with_capacity(comps.len() * 8);
    for p in &comps {
        let c = store.column(&p.name).ok_or_else(|| anyhow!("no column `{}`", p.name))?;
        let (bytes, stride) = (c.bytes().to_vec(), c.stride());
        let at = lease.put(&bytes)?;
        cols.extend_from_slice(&at.to_le_bytes());
        cols.extend_from_slice(&stride.to_le_bytes());
        let back = (p.access == Access::Write).then(|| p.name.clone());
        lent.push(Lent { back, at, len: bytes.len() as u32 });
    }

    let mut row_bytes = Vec::with_capacity(rows.len() * 4);
    for r in &rows {
        row_bytes.extend_from_slice(&r.to_le_bytes());
    }

    // The only resource is `Input`, and a stage only ever reads it — one copy,
    // however many parameters name it.
    let n_res = s.params.iter().filter(|p| p.kind == ParamKind::Resource).count() as u32;
    let mut res = Vec::with_capacity(n_res as usize * 4);
    if n_res > 0 {
        let at = lease.put(bytes_of(input))?;
        for _ in 0..n_res {
            res.extend_from_slice(&at.to_le_bytes());
        }
    }

    let cols_at = lease.put(&cols)?;
    let rows_at = lease.put(&row_bytes)?;
    let res_at = lease.put(&res)?;
    let mut call = Vec::with_capacity(size_of::<StageCall>());
    for w in [cols_at, comps.len() as u32, rows_at, n_rows, res_at, n_res] {
        call.extend_from_slice(&w.to_le_bytes());
    }
    call.extend_from_slice(&dt.to_le_bytes());
    debug_assert_eq!(call.len(), size_of::<StageCall>());
    let call_at = lease.put(&call)?;

    s.run.call1(&JsValue::NULL, &JsValue::from_f64(call_at as f64)).map_err(js_err)?;

    // The writes come home. A `&T` parameter cannot have been written — the
    // shim hands it out as `*const` — so nothing else is copied back, and a
    // read-only column is proven untouched by the signature rather than by
    // comparing bytes.
    for l in &lent {
        let Some(name) = &l.back else { continue };
        let Some(bytes) = mem.read(l.at, l.len) else {
            bail!("`{name}` did not come back out of the module's memory")
        };
        put_column(store, name, &bytes)?;
    }
    Ok(())
}

/// A column's bytes, back into the store.
///
/// The whole column goes out and the whole column comes back, so the rows the
/// query did not match are written with what they already held. A stage cannot
/// spawn, so the length cannot have moved under it.
fn put_column(store: &mut dyn Store, name: &str, bytes: &[u8]) -> Result<()> {
    let now = store.column(name).map(|c| c.bytes().len()).unwrap_or(0);
    if now != bytes.len() {
        bail!("`{name}` changed length while a stage was running");
    }
    let (base, _) = store.column_base(name).ok_or_else(|| anyhow!("no column `{name}`"))?;
    // SAFETY: `base` is this column's own storage, `bytes` is a copy of it of
    // the length just checked, and the two do not overlap — `bytes` came out
    // of the module's memory.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), base, bytes.len()) };
    Ok(())
}

/// Entities carrying every component the signature names, plus the dense index
/// into each column for each of them. `se_host::schedule`'s `resolve`.
fn resolve(store: &dyn Store, comps: &[&ParamDef]) -> (Vec<u32>, u32) {
    if comps.is_empty() {
        // No components means no query: the stage runs once.
        return (Vec::new(), 1);
    }
    let n = comps.len();
    let cols: Vec<&dyn ColumnRef> = match comps.iter().map(|p| store.column(&p.name)).collect() {
        Some(c) => c,
        // Validation proved the names are declared; a column that is not there
        // yet holds nothing, so the query matches nothing.
        None => return (Vec::new(), 0),
    };

    // Drive from the smallest column; the rest are membership tests.
    let (drive, _) = cols.iter().enumerate().min_by_key(|(_, c)| c.len()).expect("non-empty");

    let mut rows = Vec::with_capacity(cols[drive].len() * n);
    let mut slots = vec![0u32; n];
    let mut count = 0u32;
    'entity: for &e in cols[drive].entities() {
        for (i, c) in cols.iter().enumerate() {
            match c.row_of(e) {
                Some(r) => slots[i] = r,
                None => continue 'entity,
            }
        }
        rows.extend_from_slice(&slots);
        count += 1;
    }
    (rows, count)
}

/// A host-side ABI struct as the bytes the module will read.
pub(crate) fn bytes_of<T>(v: &T) -> &[u8] {
    // SAFETY: every ABI struct is `#[repr(C)]` plain data, and both sides of
    // this route are wasm32, so the module reads back what was written.
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, size_of::<T>()) }
}
