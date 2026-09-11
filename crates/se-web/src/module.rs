//! Loading a module the way wasm already loads modules.
//!
//! There is no dlopen here and nothing standing in for one. A module is
//! instantiated with `WebAssembly.instantiate` and bound to what the engine
//! exports for it. Replacing a module is instantiating another one and binding
//! it — which is what wasm is for.
//!
//! A module keeps its own linear memory, so a pointer it hands back is an
//! offset into *that* memory. Two ways of following one, and the difference
//! matters: [`WebModule::read`] takes a copy once at load and reads the
//! registrations out of it (`se_host::reloc`), while [`ModuleMem`] holds the
//! live `WebAssembly.Memory` for the calls that must see writes — control's,
//! which is `ctl.rs`. Nothing is decoded either way: both sides are wasm32
//! with the same struct layouts, which is the whole reason this route is not
//! the `.so` route with a different loader bolted on.

use anyhow::{anyhow, bail, Context, Result};
use js_sys::{Function, Object, Reflect, Uint8Array, WebAssembly};
use se_abi::{ControlSpec, Registered, Slice, Slot, SlotBind, Str};
use wasm_bindgen::{JsCast, JsValue};

/// One instantiated module, addressed through its exports.
pub struct WebModule {
    pub name: String,
    exports: Object,
}

pub(crate) fn js_err(e: JsValue) -> anyhow::Error {
    anyhow!("{}", String::from(js_sys::JSON::stringify(&e).map(String::from).unwrap_or_default()))
}

impl WebModule {
    /// Take the exports of an already-instantiated module.
    ///
    /// Instantiation is JS's job: it is what holds the fetch, the imports
    /// object and the engine's memory, and doing it from here would mean
    /// pulling all of that across the boundary twice.
    pub fn new(name: impl Into<String>, exports: Object) -> WebModule {
        WebModule { name: name.into(), exports }
    }

    pub fn has(&self, sym: &str) -> bool {
        Reflect::get(&self.exports, &JsValue::from_str(sym))
            .map(|v| v.is_function())
            .unwrap_or(false)
    }

    /// Every name this module exports, in the order the export section lists
    /// them. `stage.rs` matches a prefix against it, which is how a stage is
    /// discovered rather than named a second time.
    pub fn export_names(&self) -> Vec<String> {
        Object::keys(&self.exports).iter().filter_map(|v| v.as_string()).collect()
    }

    /// One of the module's exported functions, by name.
    ///
    /// Unlike [`WebModule::call`], which goes through the function table: an
    /// export has a name on both sides, and on wasm a stage *is* a name.
    pub fn export(&self, sym: &str) -> Result<Function> {
        self.func(sym)
    }

    fn func(&self, sym: &str) -> Result<Function> {
        let v = Reflect::get(&self.exports, &JsValue::from_str(sym)).map_err(js_err)?;
        v.dyn_into_function()
            .ok_or_else(|| anyhow!("`{}` exports no {sym}", self.name))
    }

    /// Call one `se_register_*` and take the pointer it answers with.
    ///
    /// The pointer is an offset into the shared memory, which is the engine's
    /// memory, so it is simply a pointer.
    pub fn registered_ptr(&self, sym: &str) -> Result<*const Registered> {
        let f = self.func(sym)?;
        let v = f.call0(&JsValue::NULL).map_err(js_err)?;
        let at = v.as_f64().ok_or_else(|| anyhow!("{sym} did not answer with an address"))? as u32;
        Ok(at as *const Registered)
    }

    /// The module's linear memory, copied.
    ///
    /// A module has its own memory, so a pointer it hands back is an offset
    /// into that memory and not into the engine's. Taking a copy once, after
    /// the module has registered, is what lets the engine follow those
    /// pointers with one constant added — see `se_host::reloc`. Registration
    /// happens at load, so this is paid once per module.
    pub fn memory(&self) -> Result<Vec<u8>> {
        let m = Reflect::get(&self.exports, &JsValue::from_str("memory")).map_err(js_err)?;
        let buf = Reflect::get(&m, &JsValue::from_str("buffer")).map_err(js_err)?;
        Ok(js_sys::Uint8Array::new(&buf).to_vec())
    }

    /// Everything the module registered, as the engine's own definitions.
    pub fn read(&self) -> Result<Registrations> {
        // Every entry point first, then the snapshot. A module builds its
        // registrations lazily on the first call — `hold` leaks into a
        // `OnceLock` — so memory copied before the calls does not contain
        // them, and the engine would read a module that registered nothing.
        let at_layouts = self.offset("se_register_layouts").unwrap_or(0);
        let at_buffers = self.offset("se_register_buffers").unwrap_or(0);
        let at_graph = self.offset("se_register_graph").unwrap_or(0);
        let at_assets = self.offset("se_register_assets").unwrap_or(0);

        let r = se_host::reloc::Reloc::new(self.memory()?);
        // SAFETY: the offsets are what the module answered with, and `r` holds
        // its memory as it stood after those calls.
        unsafe {
            Ok(Registrations {
                layouts: se_host::reloc_read::layouts(&r, at_layouts)?,
                buffers: se_host::reloc_read::buffers(&r, at_buffers)?,
                graphs: se_host::reloc_read::graphs(&r, at_graph)?,
                assets: se_host::reloc_read::assets(&r, at_assets)?,
            })
        }
    }

    fn offset(&self, sym: &str) -> Option<u32> {
        self.has(sym).then(|| self.registered_ptr(sym).ok()).flatten().map(|p| p as u32)
    }

    /// The module's memory and the two exports that carve space out of it.
    ///
    /// Live, unlike [`WebModule::memory`]: this is what the `se_ctl_*` imports
    /// read and write through, and a snapshot would drop every write.
    pub fn mem(&self) -> Result<ModuleMem> {
        let m = Reflect::get(&self.exports, &JsValue::from_str("memory")).map_err(js_err)?;
        let memory = m
            .dyn_into::<WebAssembly::Memory>()
            .map_err(|_| anyhow!("`{}` exports no memory", self.name))?;
        Ok(ModuleMem::new(memory, self.func("se_alloc")?, self.func("se_free")?))
    }

    /// The module's own function table, which is where its `start`, `tick` and
    /// `stop` live. A control module has to be built with
    /// `-C link-arg=--export-table` or the three are simply unreachable.
    fn table(&self) -> Result<WebAssembly::Table> {
        let t = Reflect::get(&self.exports, &JsValue::from_str("__indirect_function_table"))
            .map_err(js_err)?;
        t.dyn_into::<WebAssembly::Table>().map_err(|_| {
            anyhow!(
                "`{}` exports no __indirect_function_table — rebuild it with \
                 `-C link-arg=--export-table`",
                self.name
            )
        })
    }

    /// Call one of the module's functions by table index.
    ///
    /// This is the wasm spelling of what the `.so` route does by dereferencing
    /// the pointer: the index means something only inside the module, so the
    /// call goes *through* the module rather than out of the host.
    pub fn call(&self, index: u32, args: &[JsValue]) -> Result<JsValue> {
        let f = self.table()?.get(index).map_err(js_err)?;
        let a = js_sys::Array::new();
        for v in args {
            a.push(v);
        }
        Reflect::apply(&f, &JsValue::NULL, &a).map_err(js_err)
    }

    /// What `se_register_control` registered, or `None` for a module that is
    /// not a control module.
    pub fn control(&self) -> Result<Option<Control>> {
        if !self.has("se_register_control") {
            return Ok(None);
        }
        let at = self.registered_ptr("se_register_control")? as u32;
        let r = se_host::reloc::Reloc::new(self.memory()?);
        // SAFETY: `at` is what the module answered with, and `r` holds its
        // memory as it stood after that call.
        unsafe {
            let d = se_host::reloc_read::registered(&r, at)?;
            if d.len == 0 {
                return Ok(None);
            }
            let want = core::mem::size_of::<RawControl>() as u32;
            if d.stride != want {
                bail!(
                    "se_register_control: `{}` registered {}-byte items, this engine reads {want}",
                    self.name,
                    d.stride
                );
            }
            let items: &[RawControl] =
                r.array(d.ptr as u32, d.len).context("control runs past the module's memory")?;
            let c = &items[0];
            let slots = r.slice(&c.slots).context("control slots are outside the module")?;
            Ok(Some(Control {
                name: r.str(&c.name).context("control name is outside the module")?.to_string(),
                slots: slots
                    .iter()
                    .map(|s| {
                        Ok((
                            s.slot,
                            r.str(&s.module)
                                .context("a slot's module is outside the module")?
                                .to_string(),
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?,
                start: c.start,
                tick: c.tick,
                stop: c.stop,
            }))
        }
    }
}

/// `ControlSpec` as it sits in a module's memory.
///
/// The three function slots are table indices *there*, not addresses here, so
/// they are read as the integers they are rather than as `fn` pointers this
/// host could be tempted to call. `0` is the `None` the ABI's `Option<fn>`
/// encodes.
#[repr(C)]
#[derive(Clone, Copy)]
struct RawControl {
    name: Str,
    slots: Slice<SlotBind>,
    start: u32,
    tick: u32,
    stop: u32,
}

const _: () = assert!(core::mem::size_of::<RawControl>() == core::mem::size_of::<ControlSpec>());

/// One control module's registration, read out of its memory.
pub struct Control {
    pub name: String,
    pub slots: Vec<(Slot, String)>,
    pub start: u32,
    pub tick: u32,
    pub stop: u32,
}

/// One module's linear memory, plus the two exports that carve space out of
/// it. `se_alloc`/`se_free` exist precisely so the host can put something — a
/// `Frame`, a borrowed component — where the module can reach it.
pub struct ModuleMem {
    memory: WebAssembly::Memory,
    alloc: Function,
    free: Function,
}

impl ModuleMem {
    pub fn new(memory: WebAssembly::Memory, alloc: Function, free: Function) -> ModuleMem {
        ModuleMem { memory, alloc, free }
    }

    /// A fresh view every time. Growing a `WebAssembly.Memory` detaches the
    /// old `ArrayBuffer`, so a cached view is a corpse waiting to be read.
    fn view(&self) -> Uint8Array {
        Uint8Array::new(&self.memory.buffer())
    }

    fn fits(&self, at: u32, len: u32) -> bool {
        at != 0 && (at as u64 + len as u64) <= self.view().length() as u64
    }

    pub fn read(&self, at: u32, len: u32) -> Option<Vec<u8>> {
        if !self.fits(at, len) {
            return None;
        }
        let mut v = vec![0u8; len as usize];
        self.view().subarray(at, at + len).copy_to(&mut v);
        Some(v)
    }

    pub fn write(&self, at: u32, bytes: &[u8]) -> bool {
        let len = bytes.len() as u32;
        if !self.fits(at, len) {
            return false;
        }
        self.view().subarray(at, at + len).copy_from(bytes);
        true
    }

    /// A `Str` the module passed, flattened to `(ptr, len)` at the boundary.
    pub fn text(&self, at: u32, len: u32) -> Option<String> {
        if len == 0 {
            return Some(String::new());
        }
        String::from_utf8(self.read(at, len)?).ok()
    }

    pub fn alloc(&self, n: u32) -> u32 {
        self.alloc
            .call1(&JsValue::NULL, &JsValue::from_f64(n as f64))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as u32
    }

    pub fn free(&self, at: u32, n: u32) {
        if at != 0 {
            let _ = self.free.call2(
                &JsValue::NULL,
                &JsValue::from_f64(at as f64),
                &JsValue::from_f64(n as f64),
            );
        }
    }
}

/// What one module contributed.
pub struct Registrations {
    pub layouts: Vec<se_host::registry::LayoutDef>,
    pub buffers: Vec<se_host::registry::BufferDef>,
    pub graphs: Vec<se_host::registry::GraphDef>,
    pub assets: Vec<se_host::registry::AssetDef>,
}

/// `JsValue::dyn_into` for the one case this needs, without dragging in the
/// whole cast machinery.
trait AsFunction {
    fn dyn_into_function(self) -> Option<Function>;
}

impl AsFunction for JsValue {
    fn dyn_into_function(self) -> Option<Function> {
        self.is_function().then(|| Function::from(self))
    }
}
