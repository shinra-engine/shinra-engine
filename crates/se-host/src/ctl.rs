//! The host as `game.so` sees it.
//!
//! Every function here is one arm of the control layer's write surface, and
//! the surface is deliberately small: input, spawn/despawn, slots, save/load.
//! There is no "get me the world" call, because a wider surface is how
//! single-writer reasoning dies.

use crate::registry::AssetDef;
use crate::space::Store;
use core::ffi::c_void;
use se_abi::{Ctl, Entity, Slice, Slot, Str};

/// What a tick may ask for, collected and applied at the frame boundary.
#[derive(Default)]
pub struct Requests {
    pub slots: Vec<(Slot, String)>,
    pub swipe: i32,
    pub quit: bool,
    pub save: Option<String>,
    pub load: Option<String>,
    pub log: Vec<String>,
}

pub struct HostCtx<'a> {
    pub world: &'a mut dyn Store,
    pub assets: &'a [&'a AssetDef],
    pub req: Requests,
}

impl<'a> HostCtx<'a> {
    pub fn new(world: &'a mut dyn Store, assets: &'a [&'a AssetDef]) -> HostCtx<'a> {
        HostCtx { world, assets, req: Requests::default() }
    }

    /// The vtable handed into a tick. Borrows `self` for the call's lifetime.
    pub fn vtable(&mut self) -> Ctl {
        Ctl {
            ctx: self as *mut HostCtx as *mut c_void,
            spawn,
            despawn,
            alive,
            set,
            get,
            ptr,
            remove,
            query,
            asset,
            asset_names,
            set_slot,
            save,
            load,
            swipe,
            quit,
            log,
        }
    }
}

/// # Safety
/// `ctx` is always the `HostCtx` that produced the vtable.
unsafe fn cx<'a>(ctx: *mut c_void) -> &'a mut HostCtx<'a> {
    &mut *(ctx as *mut HostCtx)
}

unsafe extern "C" fn spawn(ctx: *mut c_void) -> Entity {
    cx(ctx).world.spawn()
}

unsafe extern "C" fn despawn(ctx: *mut c_void, e: Entity) {
    cx(ctx).world.despawn(e);
}

unsafe extern "C" fn alive(ctx: *mut c_void, e: Entity) -> bool {
    cx(ctx).world.alive(e)
}

unsafe extern "C" fn set(
    ctx: *mut c_void,
    e: Entity,
    name: Str,
    data: *const u8,
    len: u32,
) -> bool {
    let c = cx(ctx);
    let bytes = std::slice::from_raw_parts(data, len as usize);
    c.world.set(e, name.as_str(), bytes)
}

unsafe extern "C" fn get(ctx: *mut c_void, e: Entity, name: Str, out: *mut u8, cap: u32) -> bool {
    let c = cx(ctx);
    let Some(bytes) = c.world.get(e, name.as_str()) else { return false };
    if bytes.len() != cap as usize {
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
    true
}

/// A pointer straight into the column, or null. Size is checked here rather
/// than trusted: a module built against a different layout must fail to find
/// the component, not reinterpret somebody else's bytes.
unsafe extern "C" fn ptr(ctx: *mut c_void, e: Entity, name: Str, len: u32) -> *mut u8 {
    let c = cx(ctx);
    match c.world.get_mut(e, name.as_str()) {
        Some(bytes) if bytes.len() == len as usize => bytes.as_mut_ptr(),
        _ => core::ptr::null_mut(),
    }
}

unsafe extern "C" fn remove(ctx: *mut c_void, e: Entity, name: Str) -> bool {
    cx(ctx).world.remove(e, name.as_str())
}

unsafe extern "C" fn query(ctx: *mut c_void, name: Str, out: *mut Entity, cap: u32) -> u32 {
    let c = cx(ctx);
    let Some(col) = c.world.column(name.as_str()) else { return 0 };
    let ents = col.entities();
    let n = ents.len().min(cap as usize);
    std::ptr::copy_nonoverlapping(ents.as_ptr(), out, n);
    ents.len() as u32
}

unsafe extern "C" fn asset(ctx: *mut c_void, name: Str, out: *mut Slice<u8>) -> bool {
    let c = cx(ctx);
    let want = name.as_str();
    match c.assets.iter().find(|a| a.name == want) {
        Some(a) => {
            // The host owns these bytes and outlives the tick.
            *out = Slice::from_raw(a.bytes.as_ptr(), a.bytes.len());
            true
        }
        None => false,
    }
}

unsafe extern "C" fn asset_names(ctx: *mut c_void, out: *mut Str, cap: u32) -> u32 {
    let c = cx(ctx);
    for (i, a) in c.assets.iter().take(cap as usize).enumerate() {
        *out.add(i) = Str { ptr: a.name.as_ptr(), len: a.name.len() };
    }
    c.assets.len() as u32
}

unsafe extern "C" fn set_slot(ctx: *mut c_void, slot: Slot, module: Str) {
    cx(ctx).req.slots.push((slot, module.as_str().to_string()));
}

unsafe extern "C" fn save(ctx: *mut c_void, path: Str) -> bool {
    cx(ctx).req.save = Some(path.as_str().to_string());
    true
}

unsafe extern "C" fn load(ctx: *mut c_void, path: Str) -> bool {
    cx(ctx).req.load = Some(path.as_str().to_string());
    true
}

unsafe extern "C" fn swipe(ctx: *mut c_void, dir: i32) {
    cx(ctx).req.swipe = dir;
}

unsafe extern "C" fn quit(ctx: *mut c_void) {
    cx(ctx).req.quit = true;
}

unsafe extern "C" fn log(ctx: *mut c_void, msg: Str) {
    cx(ctx).req.log.push(msg.as_str().to_string());
}
