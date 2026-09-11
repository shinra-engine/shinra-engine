//! The host half of the control transport.
//!
//! A control module built for wasm32 does not receive a `Ctl` vtable — it
//! *imports* `se_ctl_spawn`, `se_ctl_set`, … by name (`se-abi`'s
//! `ctl_wasm.rs`), because a function pointer the host filled in names nothing
//! another module can call. This file is the other end of those imports.
//!
//! Two rules run through all of it:
//!
//!   * every pointer argument is an offset into the *calling module's* memory,
//!     so it is read and written through that module's live
//!     `WebAssembly.Memory` (`module::ModuleMem`) — never through the `Reloc`
//!     snapshot, which is a copy taken at load and would swallow every write.
//!   * the world behind the imports is `se-ecs` through `dyn Store`, the same
//!     one the native host drives. There is no second ECS here.
//!
//! `ctx` is not a pointer. It is a token the host minted before the tick; it
//! selects which world and which memory a call is about, which is the one
//! thing a raw `ctx` pointer could never do across a module boundary.

use crate::module::ModuleMem;
use js_sys::{Function, Object, Reflect};
use se_abi::{Entity, Slot};
use se_host::registry::AssetDef;
use se_host::space::Store;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

/// What a tick asked for, collected and applied at the frame boundary.
///
/// The same list `se_host::ctl::Requests` keeps. It is not reused because that
/// module is the `.so` route's vtable and is compiled out on wasm32.
#[derive(Default)]
pub struct Requests {
    pub slots: Vec<(Slot, String)>,
    pub swipe: i32,
    pub quit: bool,
    pub save: Option<String>,
    pub load: Option<String>,
    pub log: Vec<String>,
}

/// A component `se_ctl_ptr` lent out.
///
/// Natively that call answers with a pointer straight into the column and the
/// module's writes land as they happen. Here the column is in the *host's*
/// memory, which the module cannot address at all, so the bytes are copied
/// into the module's memory and copied back when the tick returns — the same
/// bargain the stage path already makes (`se`'s `se_alloc`). What a game could
/// observe is that a `get` of the same component later in the same tick still
/// reads the old value; nothing in the three examples does that.
struct Borrow {
    e: Entity,
    name: String,
    at: u32,
    len: u32,
}

/// The world one module ticks against, and the memory it speaks through.
pub struct Host {
    pub store: Box<dyn Store>,
    /// The bundle's, shared with the graph that draws them — the same
    /// `Rc` `render::Loaded` holds, so lending an asset to a module costs one
    /// copy into that module and nothing on this side.
    pub assets: Rc<Vec<AssetDef>>,
    pub req: Requests,
    pub mem: ModuleMem,
    borrows: Vec<Borrow>,
    lent_assets: HashMap<String, (u32, u32)>,
}

impl Host {
    pub fn new(store: Box<dyn Store>, assets: Rc<Vec<AssetDef>>, mem: ModuleMem) -> Host {
        Host {
            store,
            assets,
            req: Requests::default(),
            mem,
            borrows: Vec::new(),
            lent_assets: HashMap::new(),
        }
    }

    fn borrow(&mut self, e: Entity, name: &str, size: u32) -> u32 {
        if let Some(b) = self.borrows.iter().find(|b| b.e == e && b.name == name) {
            return b.at;
        }
        let Some(cur) = self.store.get(e, name) else { return 0 };
        if cur.len() != size as usize {
            return 0;
        }
        let cur = cur.to_vec();
        let at = self.mem.alloc(size);
        if at == 0 || !self.mem.write(at, &cur) {
            self.mem.free(at, size);
            return 0;
        }
        self.borrows.push(Borrow { e, name: name.to_string(), at, len: size });
        at
    }

    /// End of tick: everything lent out goes back into the world.
    pub fn flush(&mut self) {
        for b in std::mem::take(&mut self.borrows) {
            if let Some(bytes) = self.mem.read(b.at, b.len) {
                self.store.set(b.e, &b.name, &bytes);
            }
            self.mem.free(b.at, b.len);
        }
    }

    /// Asset bytes, placed where the module can read them.
    ///
    /// The wrapper hands the game a `&'static [u8]`, so the copy has to
    /// outlive the tick; it is kept for as long as this host is and shared by
    /// every later ask for the same name.
    fn lend_asset(&mut self, name: &str) -> Option<(u32, u32)> {
        if let Some(&at) = self.lent_assets.get(name) {
            return Some(at);
        }
        let bytes = self.assets.iter().find(|a| a.name == name)?.bytes.clone();
        let len = bytes.len() as u32;
        let at = self.mem.alloc(len.max(1));
        if at == 0 || !self.mem.write(at, &bytes) {
            return None;
        }
        self.lent_assets.insert(name.to_string(), (at, len));
        Some((at, len))
    }
}

thread_local! {
    /// Handle → world. A handle is an index plus one, so `0` is never live and
    /// a module that was handed nothing cannot address a world by accident.
    static HOSTS: RefCell<Vec<Option<Host>>> = const { RefCell::new(Vec::new()) };
}

pub fn install(h: Host) -> u32 {
    HOSTS.with(|hs| {
        let mut hs = hs.borrow_mut();
        hs.push(Some(h));
        hs.len() as u32
    })
}

/// Take the world back out. The imports stop resolving for this handle, which
/// is what makes a call after the tick loud rather than silently effective.
pub fn reclaim(ctx: u32) -> Option<Host> {
    HOSTS.with(|hs| hs.borrow_mut().get_mut(ctx.checked_sub(1)? as usize)?.take())
}

pub fn flush(ctx: u32) {
    on("flush", ctx, |h| h.flush())
}

/// The world behind `ctx`, for the length of one call.
///
/// A tick's world lives in this registry, not in the `Game` handle, so the
/// stage path reaches it the same way the `se_ctl_*` imports do. `None` says
/// the handle names no world, which the caller must not silently ignore.
pub fn with_store<R>(ctx: u32, f: impl FnOnce(&mut dyn Store) -> R) -> Option<R> {
    HOSTS.with(|hs| {
        let mut hs = hs.borrow_mut();
        match hs.get_mut(ctx.checked_sub(1)? as usize) {
            Some(Some(h)) => Some(f(&mut *h.store)),
            _ => None,
        }
    })
}

/// Run `f` against the world `ctx` names, or say so and answer with nothing.
///
/// The registry borrow is taken and dropped inside this call and never held
/// across one into the module, so a tick calling back mid-call cannot find it
/// already borrowed.
fn on<R: Default>(what: &str, ctx: u32, f: impl FnOnce(&mut Host) -> R) -> R {
    let got = HOSTS.with(|hs| {
        let mut hs = hs.borrow_mut();
        let i = ctx.checked_sub(1)? as usize;
        match hs.get_mut(i) {
            Some(Some(h)) => Some(f(h)),
            _ => None,
        }
    });
    got.unwrap_or_else(|| {
        web_sys::console::error_1(&JsValue::from_str(&format!(
            "{what}: no world behind ctl handle {ctx} — the call was dropped"
        )));
        R::default()
    })
}

/// Bind one import into `env`. The second arm is the same thing for the arms
/// that answer nothing.
macro_rules! put {
    ($env:expr, $name:literal, |$($a:ident : $t:ty),*| -> $r:ty $body:block) => {
        put!(@bind $env, $name, Box::new(move |$($a: $t),*| -> $r { $body })
            as Box<dyn Fn($($t),*) -> $r>)
    };
    ($env:expr, $name:literal, |$($a:ident : $t:ty),*| $body:block) => {
        put!(@bind $env, $name, Box::new(move |$($a: $t),*| { $body })
            as Box<dyn Fn($($t),*)>)
    };
    (@bind $env:expr, $name:literal, $closure:expr) => {{
        let f = Closure::wrap($closure);
        let _ = Reflect::set(
            $env,
            &JsValue::from_str($name),
            f.as_ref().unchecked_ref::<Function>(),
        );
        // The import set outlives every module instantiated against it, and
        // `engineImports` builds it once, so this leaks one closure per name
        // for the life of the page rather than one per module.
        f.forget();
    }};
}

/// Every `se_ctl_*` a module can import, bound into the `env` namespace.
///
/// One entry per import `ctl_wasm.rs` declares, and no more. The native
/// vtable's `asset_names` slot has no wrapper and no import on either route,
/// so there is nothing here for it to answer.
pub fn imports(env: &Object) {
    put!(env, "se_ctl_spawn", |ctx: u32| -> u64 {
        on("se_ctl_spawn", ctx, |h| h.store.spawn())
    });
    put!(env, "se_ctl_despawn", |ctx: u32, e: u64| {
        on("se_ctl_despawn", ctx, |h| h.store.despawn(e));
    });
    put!(env, "se_ctl_alive", |ctx: u32, e: u64| -> bool {
        on("se_ctl_alive", ctx, |h| h.store.alive(e))
    });
    put!(env, "se_ctl_set",
        |ctx: u32, e: u64, n: u32, n_len: u32, v: u32, size: u32| -> bool {
        on("se_ctl_set", ctx, |h| {
            let (Some(name), Some(bytes)) = (h.mem.text(n, n_len), h.mem.read(v, size)) else {
                return false;
            };
            h.store.set(e, &name, &bytes)
        })
    });
    put!(env, "se_ctl_get",
        |ctx: u32, e: u64, n: u32, n_len: u32, out: u32, size: u32| -> bool {
        on("se_ctl_get", ctx, |h| {
            let Some(name) = h.mem.text(n, n_len) else { return false };
            let Some(bytes) = h.store.get(e, &name).map(|b| b.to_vec()) else { return false };
            bytes.len() == size as usize && h.mem.write(out, &bytes)
        })
    });
    put!(env, "se_ctl_ptr", |ctx: u32, e: u64, n: u32, n_len: u32, size: u32| -> u32 {
        on("se_ctl_ptr", ctx, |h| match h.mem.text(n, n_len) {
            Some(name) => h.borrow(e, &name, size),
            None => 0,
        })
    });
    put!(env, "se_ctl_remove", |ctx: u32, e: u64, n: u32, n_len: u32| -> bool {
        on("se_ctl_remove", ctx, |h| match h.mem.text(n, n_len) {
            Some(name) => h.store.remove(e, &name),
            None => false,
        })
    });
    put!(env, "se_ctl_query", |ctx: u32, n: u32, n_len: u32, out: u32, cap: u32| -> u32 {
        on("se_ctl_query", ctx, |h| {
            let Some(name) = h.mem.text(n, n_len) else { return 0 };
            let Some(col) = h.store.column(&name) else { return 0 };
            let ents = col.entities();
            let mut buf = Vec::new();
            for e in ents.iter().take(cap as usize) {
                buf.extend_from_slice(&e.to_le_bytes());
            }
            let total = ents.len() as u32;
            if !buf.is_empty() && !h.mem.write(out, &buf) {
                return 0;
            }
            total
        })
    });
    put!(env, "se_ctl_asset", |ctx: u32, n: u32, n_len: u32, out: u32| -> bool {
        on("se_ctl_asset", ctx, |h| {
            let Some(name) = h.mem.text(n, n_len) else { return false };
            let Some((at, len)) = h.lend_asset(&name) else { return false };
            // `Slice<u8>` on wasm32 is two 32-bit words, and `ptr` is an
            // offset in the module's memory because that is where the copy is.
            let mut s = [0u8; 8];
            s[..4].copy_from_slice(&at.to_le_bytes());
            s[4..].copy_from_slice(&len.to_le_bytes());
            h.mem.write(out, &s)
        })
    });
    requests(env);
}

/// The arms that only record what the frame boundary must act on.
fn requests(env: &Object) {
    put!(env, "se_ctl_set_slot", |ctx: u32, slot: u32, n: u32, n_len: u32| {
        on("se_ctl_set_slot", ctx, |h| {
            let (Some(name), Some(slot)) = (h.mem.text(n, n_len), slot_of(slot)) else { return };
            h.req.slots.push((slot, name));
        });
    });
    put!(env, "se_ctl_save", |ctx: u32, n: u32, n_len: u32| -> bool {
        on("se_ctl_save", ctx, |h| {
            h.req.save = h.mem.text(n, n_len);
            h.req.save.is_some()
        })
    });
    put!(env, "se_ctl_load", |ctx: u32, n: u32, n_len: u32| -> bool {
        on("se_ctl_load", ctx, |h| {
            h.req.load = h.mem.text(n, n_len);
            h.req.load.is_some()
        })
    });
    put!(env, "se_ctl_swipe", |ctx: u32, dir: i32| {
        on("se_ctl_swipe", ctx, |h| h.req.swipe = dir);
    });
    put!(env, "se_ctl_quit", |ctx: u32| {
        on("se_ctl_quit", ctx, |h| h.req.quit = true);
    });
    put!(env, "se_ctl_log", |ctx: u32, msg: u32, msg_len: u32| {
        on("se_ctl_log", ctx, |h| {
            if let Some(m) = h.mem.text(msg, msg_len) {
                h.req.log.push(m);
            }
        });
    });
}

/// `Slot` is `#[repr(u32)]`, but a number off the wire is not yet one of them.
fn slot_of(n: u32) -> Option<Slot> {
    match n {
        0 => Some(Slot::Asset),
        1 => Some(Slot::Process),
        2 => Some(Slot::Render),
        _ => None,
    }
}
