//! The wasm32 module-loading path: a browser `WebAssembly.Module`/`Instance`
//! stand in for wasmtime's `Module`/`Instance`, per story-229/task-237's T2
//! design table. There is no `wasmtime::Engine` reachable from inside a
//! wasm32 binary (T1's finding), so this is not a thinner wrapper the way
//! `mem::web::WebMem` can lean on a `Memory` the host already shares — it is
//! a second implementation of the same four jobs `module::native` does:
//! compile + check, instantiate, call an export, and enumerate exports.
//!
//! One job native has no counterpart for here: fuel budgeting
//! (`with_budget`, `DEFAULT_FUEL`). The Web API has no equivalent — a
//! runaway stage can only hang the tab until the browser itself intervenes.
//! That is an accepted behavior change for the web target (story-229's T2
//! design), not something this file works around.

use super::common::check_contract_id;
use crate::memory::Slot;
use anyhow::{bail, Context, Result};
use js_sys::wasm_bindgen::{JsCast, JsValue};
use js_sys::{Function, Object, Reflect, Uint8Array, WebAssembly};

pub struct Loaded {
    pub name: String,
    pub slot: Slot,
    pub instance: WebAssembly::Instance,
    /// The file, kept so its custom sections stay readable after load.
    pub bytes: Vec<u8>,
}

/// A module compiled and checked, but not instantiated: nothing it carries
/// has touched the shared memory yet, so refusing it costs nothing.
pub struct Prepared {
    pub name: String,
    pub slot: Slot,
    module: WebAssembly::Module,
    bytes: Vec<u8>,
}

impl Prepared {
    pub fn section(&self, name: &str) -> Result<&[u8]> {
        se_contract::wasm::section(&self.bytes, name).with_context(|| format!("in `{}`", self.name))
    }

    pub fn has_export(&self, sym: &str) -> bool {
        WebAssembly::Module::exports(&self.module)
            .iter()
            .filter_map(|e| Reflect::get(&e, &JsValue::from_str("name")).ok())
            .filter_map(|n| n.as_string())
            .any(|n| n == sym)
    }
}

/// Everything a module is allowed to ask the host for.
const MEMORY_IMPORT: (&str, &str) = ("env", "memory");

fn js_err(what: &str, e: JsValue) -> anyhow::Error {
    anyhow::anyhow!(
        "{what}: {}",
        js_sys::wasm_bindgen::JsValue::as_string(&e).unwrap_or_else(|| format!("{e:?}"))
    )
}

fn check_shape(name: &str, m: &WebAssembly::Module) -> Result<()> {
    let mut imports = Vec::new();
    let mut has_memory_import = false;
    for i in WebAssembly::Module::imports(m).iter() {
        let module = Reflect::get(&i, &JsValue::from_str("module"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let field = Reflect::get(&i, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let kind = Reflect::get(&i, &JsValue::from_str("kind"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        if (module.as_str(), field.as_str()) == MEMORY_IMPORT && kind == "memory" {
            has_memory_import = true;
            continue;
        }
        imports.push(format!("{module}::{field}"));
    }
    if !imports.is_empty() {
        bail!(
            "`{name}` imports {imports:?}. A module may import only `env::memory`: \
             an import that re-enters another module would put two of them on the \
             one shared stack at once."
        )
    }
    if !has_memory_import {
        bail!(
            "`{name}` imports no memory — it was built without `--import-memory`, so \
             nothing it writes would reach the world"
        )
    }
    let exports_own_memory = WebAssembly::Module::exports(m).iter().any(|e| {
        Reflect::get(&e, &JsValue::from_str("kind"))
            .ok()
            .and_then(|v| v.as_string())
            .as_deref()
            == Some("memory")
    });
    if exports_own_memory {
        bail!("`{name}` exports its own memory; it is not sharing the host's")
    }
    Ok(())
}

/// The entry point a slot calls every frame, as (symbol, how the refusal
/// spells it). A process module's stages are checked against its own
/// declaration instead.
fn entry(slot: Slot) -> Option<(&'static str, &'static str)> {
    match slot {
        Slot::Game => Some(("tick", "tick(u32)")),
        Slot::Render => Some(("record", "record()")),
        Slot::Process => None,
    }
}

/// Checked on the compiled module, before any instance exists: a game with no
/// `tick` would otherwise replace the running one and fail every frame.
///
/// Unlike native's `check_entry`, `WebAssembly.Module.exports` reflection
/// gives only a function's `kind`, never its param/result arity — the arity
/// check native does with `wasmtime::ValType` has no web-side counterpart and
/// is instead left to `commit()`'s `get_typed_func`-equivalent call, which
/// fails at call time if the export is not actually `(u32) -> u32`.
fn check_entry(name: &str, slot: Slot, m: &WebAssembly::Module) -> Result<()> {
    let Some((sym, shown)) = entry(slot) else {
        return Ok(());
    };
    let ok = WebAssembly::Module::exports(m).iter().any(|e| {
        let export_name = Reflect::get(&e, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string());
        let kind = Reflect::get(&e, &JsValue::from_str("kind"))
            .ok()
            .and_then(|v| v.as_string());
        export_name.as_deref() == Some(sym) && kind.as_deref() == Some("function")
    });
    if !ok {
        bail!(
            "`{name}` exports no {shown} -> u32; the {} slot calls it every frame",
            slot.name()
        )
    }
    Ok(())
}

impl Loaded {
    /// Compile and check. No instance and no memory are in reach, so a
    /// refusal here cannot have changed anything.
    pub fn prepare(slot: Slot, name: &str, bytes: Vec<u8>, contract_id: u64) -> Result<Prepared> {
        let array = Uint8Array::from(bytes.as_slice());
        let module = WebAssembly::Module::new(&array)
            .map_err(|e| js_err(&format!("`{name}` is not a wasm module"), e))?;
        check_shape(name, &module)?;
        check_entry(name, slot, &module)?;
        check_contract_id(name, &bytes, contract_id)?;
        Ok(Prepared {
            name: name.to_string(),
            slot,
            module,
            bytes,
        })
    }

    /// Instantiate: from here the module's data segments are in the memory.
    pub fn commit(p: Prepared, memory: &WebAssembly::Memory) -> Result<Loaded> {
        let env = Object::new();
        Reflect::set(&env, &JsValue::from_str(MEMORY_IMPORT.1), memory)
            .map_err(|e| js_err(&format!("defining `env::memory` for `{}`", p.name), e))?;
        let imports = Object::new();
        Reflect::set(&imports, &JsValue::from_str(MEMORY_IMPORT.0), &env)
            .map_err(|e| js_err(&format!("building the import object for `{}`", p.name), e))?;
        // Instantiation runs a start function, so it is the second way in.
        let instance = WebAssembly::Instance::new(&p.module, &imports).map_err(|e| {
            js_err(
                &format!("instantiating `{}` into the {} slot", p.name, p.slot.name()),
                e,
            )
        })?;
        Ok(Loaded {
            name: p.name,
            slot: p.slot,
            instance,
            bytes: p.bytes,
        })
    }

    /// A section the module carries as data — `se.wgsl` for a render module,
    /// `se.asset` for content. Read from the file, so no instantiation is
    /// needed to get at it.
    pub fn section(&self, name: &str) -> Result<&[u8]> {
        se_contract::wasm::section(&self.bytes, name).with_context(|| format!("in `{}`", self.name))
    }

    fn export(&self, sym: &str) -> Result<Function> {
        let exports = WebAssembly::Instance::exports(&self.instance);
        let v = Reflect::get(&exports, &JsValue::from_str(sym))
            .map_err(|e| js_err(&format!("`{}` exports no {sym}", self.name), e))?;
        v.dyn_into::<Function>()
            .map_err(|_| anyhow::anyhow!("`{}` exports no {sym}()", self.name))
    }

    /// Call a zero-argument, `u32`-returning export. The web target has no
    /// `Store` to thread through (that is a wasmtime concept), so unlike
    /// native's `func0` this resolves and calls the export in one step
    /// rather than handing back a reusable `TypedFunc`.
    pub fn func0(&self, sym: &str) -> Result<u32> {
        let f = self.export(sym)?;
        let r = f
            .call0(&JsValue::NULL)
            .map_err(|e| js_err(&format!("calling `{}`'s {sym}()", self.name), e))?;
        r.as_f64()
            .map(|n| n as u32)
            .with_context(|| format!("`{}`'s {sym}() did not return a number", self.name))
    }

    /// Call a one-argument (`u32 -> u32`) export. See `func0` for why this
    /// calls immediately rather than returning a `TypedFunc`-equivalent.
    pub fn func1(&self, sym: &str, arg: u32) -> Result<u32> {
        let f = self.export(sym)?;
        let r = f
            .call1(&JsValue::NULL, &JsValue::from_f64(arg as f64))
            .map_err(|e| js_err(&format!("calling `{}`'s {sym}({arg})", self.name), e))?;
        r.as_f64()
            .map(|n| n as u32)
            .with_context(|| format!("`{}`'s {sym}({arg}) did not return a number", self.name))
    }

    /// Every export whose name begins with `prefix`.
    ///
    /// This is how a stage is found rather than listed: `contract.wit` closes
    /// the set of *types*, and a process module may export any number of
    /// stages over them, including ones written long after this host was
    /// built.
    pub fn exports_with(&self, prefix: &str) -> Vec<String> {
        let exports = WebAssembly::Instance::exports(&self.instance);
        let mut v: Vec<String> = Object::keys(&exports)
            .iter()
            .filter_map(|k| k.as_string())
            .filter(|n| n.starts_with(prefix))
            .collect();
        v.sort();
        v
    }
}
