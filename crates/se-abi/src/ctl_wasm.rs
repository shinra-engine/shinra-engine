//! `Ctl` on the wasm route: imported host functions, not a vtable.
//!
//! A vtable cannot cross a wasm module boundary. A function pointer there is
//! an index into the *calling* module's table and `ctx` is an offset into the
//! *calling* module's memory, so a `Ctl` the host filled in names functions it
//! alone can call and memory it alone can read. It would compile and be wrong
//! at run time.
//!
//! The engine has already answered this twice. Registration inverted the sink
//! so the host reads the module (`reg.rs`); the allocator went the other way
//! and had the module *import* the host by name (`se`'s `heap.rs`). Control is
//! the second direction, so it takes the second answer: the calls below are
//! `se_ctl_*` imports the linker records and `se-web` supplies at
//! instantiation.
//!
//! Two consequences worth naming:
//!
//!   * `Str` is flattened into `(ptr, len)` at the boundary. A wasm import
//!     takes scalars, and passing a `#[repr(C)]` struct by value across a
//!     module boundary leans on ABI detail there is no reason to lean on.
//!   * every pointer here stays an offset into *this* module's memory. The
//!     host reads and writes it through the module's `Memory`; it never
//!     dereferences it as one of its own.
//!
//! There is one import per method a game can call, and no more. The native
//! vtable's `asset_names` slot has no wrapper on either route, so there is
//! nothing here that could reach it and nothing to import.

use crate::control::Entity;
use crate::input::Slot;
use crate::prim::{Slice, Str};

extern "C" {
    fn se_ctl_spawn(ctx: u32) -> Entity;
    fn se_ctl_despawn(ctx: u32, e: Entity);
    fn se_ctl_alive(ctx: u32, e: Entity) -> bool;
    fn se_ctl_set(ctx: u32, e: Entity, n: *const u8, n_len: usize, v: *const u8, size: u32)
        -> bool;
    fn se_ctl_get(ctx: u32, e: Entity, n: *const u8, n_len: usize, out: *mut u8, size: u32)
        -> bool;
    fn se_ctl_ptr(ctx: u32, e: Entity, n: *const u8, n_len: usize, size: u32) -> *mut u8;
    fn se_ctl_remove(ctx: u32, e: Entity, n: *const u8, n_len: usize) -> bool;
    fn se_ctl_query(ctx: u32, n: *const u8, n_len: usize, out: *mut Entity, cap: u32) -> u32;
    fn se_ctl_asset(ctx: u32, n: *const u8, n_len: usize, out: *mut Slice<u8>) -> bool;
    fn se_ctl_set_slot(ctx: u32, slot: u32, n: *const u8, n_len: usize);
    fn se_ctl_save(ctx: u32, n: *const u8, n_len: usize) -> bool;
    fn se_ctl_load(ctx: u32, n: *const u8, n_len: usize) -> bool;
    fn se_ctl_swipe(ctx: u32, dir: i32);
    fn se_ctl_quit(ctx: u32);
    fn se_ctl_log(ctx: u32, msg: *const u8, msg_len: usize);
}

/// The host, as seen from inside a tick.
///
/// One field, and it is not a pointer: there is nothing in the module's own
/// memory for the host to point at. `ctx` is whatever token the host handed
/// this module — a world handle — and the module only ever passes it back.
#[repr(C)]
pub struct Ctl {
    pub ctx: u32,
}

/// The transport. One arm per import, each flattening its `Str` on the way.
///
/// `control.rs` writes the method surface once against exactly these; the two
/// routes differ here and nowhere else.
impl Ctl {
    pub(crate) fn raw_spawn(&mut self) -> Entity {
        unsafe { se_ctl_spawn(self.ctx) }
    }
    pub(crate) fn raw_despawn(&mut self, e: Entity) {
        unsafe { se_ctl_despawn(self.ctx, e) }
    }
    pub(crate) fn raw_alive(&mut self, e: Entity) -> bool {
        unsafe { se_ctl_alive(self.ctx, e) }
    }
    /// # Safety
    /// `v` reads `size` bytes.
    pub(crate) unsafe fn raw_set(&mut self, e: Entity, n: Str, v: *const u8, size: u32) -> bool {
        se_ctl_set(self.ctx, e, n.ptr, n.len, v, size)
    }
    /// # Safety
    /// `out` is written for `size` bytes.
    pub(crate) unsafe fn raw_get(&mut self, e: Entity, n: Str, out: *mut u8, size: u32) -> bool {
        se_ctl_get(self.ctx, e, n.ptr, n.len, out, size)
    }
    pub(crate) fn raw_ptr(&mut self, e: Entity, n: Str, size: u32) -> *mut u8 {
        unsafe { se_ctl_ptr(self.ctx, e, n.ptr, n.len, size) }
    }
    pub(crate) fn raw_remove(&mut self, e: Entity, n: Str) -> bool {
        unsafe { se_ctl_remove(self.ctx, e, n.ptr, n.len) }
    }
    /// # Safety
    /// `out` holds `cap` entities.
    pub(crate) unsafe fn raw_query(&mut self, n: Str, out: *mut Entity, cap: u32) -> u32 {
        se_ctl_query(self.ctx, n.ptr, n.len, out, cap)
    }
    /// # Safety
    /// `out` is a live `Slice<u8>`. What the host writes into it is an offset
    /// in this module's memory, like every other pointer crossing here.
    pub(crate) unsafe fn raw_asset(&mut self, n: Str, out: *mut Slice<u8>) -> bool {
        se_ctl_asset(self.ctx, n.ptr, n.len, out)
    }
    pub(crate) fn raw_set_slot(&mut self, slot: Slot, module: Str) {
        unsafe { se_ctl_set_slot(self.ctx, slot as u32, module.ptr, module.len) }
    }
    pub(crate) fn raw_save(&mut self, name: Str) -> bool {
        unsafe { se_ctl_save(self.ctx, name.ptr, name.len) }
    }
    pub(crate) fn raw_load(&mut self, name: Str) -> bool {
        unsafe { se_ctl_load(self.ctx, name.ptr, name.len) }
    }
    pub(crate) fn raw_swipe(&mut self, dir: i32) {
        unsafe { se_ctl_swipe(self.ctx, dir) }
    }
    pub(crate) fn raw_quit(&mut self) {
        unsafe { se_ctl_quit(self.ctx) }
    }
    pub(crate) fn raw_log(&mut self, msg: Str) {
        unsafe { se_ctl_log(self.ctx, msg.ptr, msg.len) }
    }
}
