//! `Ctl` on the `.so` route: a vtable of host function pointers.
//!
//! Host and module share one address space here, so a function pointer names
//! a function and `ctx` names memory. That is the whole reason this shape is
//! right on this route and impossible on the other one — see `ctl_wasm.rs`.
//!
//! A vtable rather than a trait object so the shape survives being compiled by
//! a different rustc.

use crate::control::Entity;
use crate::input::Slot;
use crate::prim::{Slice, Str};

/// The host, as seen from inside a tick.
#[repr(C)]
pub struct Ctl {
    pub ctx: *mut core::ffi::c_void,

    pub spawn: unsafe extern "C" fn(*mut core::ffi::c_void) -> Entity,
    pub despawn: unsafe extern "C" fn(*mut core::ffi::c_void, Entity),
    pub alive: unsafe extern "C" fn(*mut core::ffi::c_void, Entity) -> bool,

    /// Write a component. Refused if size disagrees with the bundle layout.
    pub set: unsafe extern "C" fn(*mut core::ffi::c_void, Entity, Str, *const u8, u32) -> bool,
    /// Read a component into caller memory.
    pub get: unsafe extern "C" fn(*mut core::ffi::c_void, Entity, Str, *mut u8, u32) -> bool,
    /// Borrow a component in place: a pointer straight into the column, or
    /// null if the entity does not carry it or the size disagrees.
    ///
    /// This is what lets control edit a component without the read-modify-
    /// write round trip, and with it the whole class of bugs where the write
    /// back was forgotten and the change silently vanished. Validity ends at
    /// the next world mutation, which the safe wrapper enforces by borrowing
    /// `Ctl` for as long as the reference lives.
    pub ptr: unsafe extern "C" fn(*mut core::ffi::c_void, Entity, Str, u32) -> *mut u8,
    pub remove: unsafe extern "C" fn(*mut core::ffi::c_void, Entity, Str) -> bool,

    /// Every entity carrying a component, written into `out`; returns the true
    /// count, which may exceed `cap`.
    pub query: unsafe extern "C" fn(*mut core::ffi::c_void, Str, *mut Entity, u32) -> u32,

    /// Content, read-only. Definitions live here; spawning from them is the
    /// control layer's job, never the asset's.
    pub asset: unsafe extern "C" fn(*mut core::ffi::c_void, Str, *mut Slice<u8>) -> bool,
    /// Names of every loaded asset, for reconciling a roster against content.
    pub asset_names: unsafe extern "C" fn(*mut core::ffi::c_void, *mut Str, u32) -> u32,

    /// Repoint a slot. Takes effect at the next frame boundary.
    pub set_slot: unsafe extern "C" fn(*mut core::ffi::c_void, Slot, Str),

    /// Status, not roster: what happened, so it can happen again.
    pub save: unsafe extern "C" fn(*mut core::ffi::c_void, Str) -> bool,
    pub load: unsafe extern "C" fn(*mut core::ffi::c_void, Str) -> bool,

    /// GameTok: hand the deck to the next or previous bundle.
    pub swipe: unsafe extern "C" fn(*mut core::ffi::c_void, i32),
    pub quit: unsafe extern "C" fn(*mut core::ffi::c_void),
    pub log: unsafe extern "C" fn(*mut core::ffi::c_void, Str),
}

/// The transport. One arm per vtable slot, each a straight indirect call.
///
/// `control.rs` writes the method surface once against exactly these; the two
/// routes differ here and nowhere else.
impl Ctl {
    pub(crate) fn raw_spawn(&mut self) -> Entity {
        unsafe { (self.spawn)(self.ctx) }
    }
    pub(crate) fn raw_despawn(&mut self, e: Entity) {
        unsafe { (self.despawn)(self.ctx, e) }
    }
    pub(crate) fn raw_alive(&mut self, e: Entity) -> bool {
        unsafe { (self.alive)(self.ctx, e) }
    }
    /// # Safety
    /// `v` reads `size` bytes.
    pub(crate) unsafe fn raw_set(&mut self, e: Entity, n: Str, v: *const u8, size: u32) -> bool {
        (self.set)(self.ctx, e, n, v, size)
    }
    /// # Safety
    /// `out` is written for `size` bytes.
    pub(crate) unsafe fn raw_get(&mut self, e: Entity, n: Str, out: *mut u8, size: u32) -> bool {
        (self.get)(self.ctx, e, n, out, size)
    }
    pub(crate) fn raw_ptr(&mut self, e: Entity, n: Str, size: u32) -> *mut u8 {
        unsafe { (self.ptr)(self.ctx, e, n, size) }
    }
    pub(crate) fn raw_remove(&mut self, e: Entity, n: Str) -> bool {
        unsafe { (self.remove)(self.ctx, e, n) }
    }
    /// # Safety
    /// `out` holds `cap` entities.
    pub(crate) unsafe fn raw_query(&mut self, n: Str, out: *mut Entity, cap: u32) -> u32 {
        (self.query)(self.ctx, n, out, cap)
    }
    /// # Safety
    /// `out` is a live `Slice<u8>`.
    pub(crate) unsafe fn raw_asset(&mut self, n: Str, out: *mut Slice<u8>) -> bool {
        (self.asset)(self.ctx, n, out)
    }
    pub(crate) fn raw_set_slot(&mut self, slot: Slot, module: Str) {
        unsafe { (self.set_slot)(self.ctx, slot, module) }
    }
    pub(crate) fn raw_save(&mut self, name: Str) -> bool {
        unsafe { (self.save)(self.ctx, name) }
    }
    pub(crate) fn raw_load(&mut self, name: Str) -> bool {
        unsafe { (self.load)(self.ctx, name) }
    }
    pub(crate) fn raw_swipe(&mut self, dir: i32) {
        unsafe { (self.swipe)(self.ctx, dir) }
    }
    pub(crate) fn raw_quit(&mut self) {
        unsafe { (self.quit)(self.ctx) }
    }
    pub(crate) fn raw_log(&mut self, msg: Str) {
        unsafe { (self.log)(self.ctx, msg) }
    }
}
