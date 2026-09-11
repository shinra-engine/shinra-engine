//! `game/*.so`: the only station that knows the time.
//!
//! It owns time, input, and which module fills which slot. Its write surface
//! is deliberately two things wide — the input components, and spawn/despawn
//! reconciling what should exist against what does. Everything else it can
//! only read.
//!
//! What a game writes against is the `impl Ctl` below, and it is written once.
//! The struct underneath it is not: on the `.so` route `Ctl` is a vtable of
//! host function pointers (`ctl_native.rs`), on the wasm route it is a handle
//! plus imported host functions (`ctl_wasm.rs`), because a function pointer
//! means nothing across a wasm module boundary. Same methods, same signatures,
//! different transport — a game never learns which side it was compiled for.

use crate::data::Schema;
use crate::input::{Frame, Slot, SlotBind};
use crate::prim::{Slice, Str};

#[cfg(not(target_arch = "wasm32"))]
pub use crate::ctl_native::Ctl;
#[cfg(target_arch = "wasm32")]
pub use crate::ctl_wasm::Ctl;

pub type Entity = u64;
pub const NO_ENTITY: Entity = 0;

/// Ergonomic wrappers. Every one is a straight call into the route's
/// transport, and nothing here knows which route that is.
impl Ctl {
    pub fn spawn(&mut self) -> Entity {
        self.raw_spawn()
    }
    pub fn despawn(&mut self, e: Entity) {
        self.raw_despawn(e)
    }
    pub fn alive(&mut self, e: Entity) -> bool {
        self.raw_alive(e)
    }
    pub fn set<T: Copy>(&mut self, e: Entity, name: &'static str, v: &T) -> bool {
        unsafe {
            self.raw_set(
                e,
                Str::new(name),
                v as *const T as *const u8,
                core::mem::size_of::<T>() as u32,
            )
        }
    }
    pub fn get<T: Copy>(&mut self, e: Entity, name: &'static str) -> Option<T> {
        let mut v = core::mem::MaybeUninit::<T>::uninit();
        let ok = unsafe {
            self.raw_get(
                e,
                Str::new(name),
                v.as_mut_ptr() as *mut u8,
                core::mem::size_of::<T>() as u32,
            )
        };
        if ok {
            Some(unsafe { v.assume_init() })
        } else {
            None
        }
    }
    /// Borrow a component in place. Writes go straight to the world.
    ///
    /// The component names itself — `Schema::NAME` comes from the same
    /// `derive` that produced the layout — so a typo is a compile error rather
    /// than a lookup that quietly returns nothing.
    pub fn get_mut<T: Schema>(&mut self, e: Entity) -> Option<&mut T> {
        let p = self.raw_ptr(e, Str::new(T::NAME), core::mem::size_of::<T>() as u32);
        if p.is_null() {
            return None;
        }
        // SAFETY: the host returned a pointer to a live `T` in its column, and
        // the borrow of `self` keeps any spawn or despawn — the only things
        // that could move it — from happening while this reference lives.
        Some(unsafe { &mut *(p as *mut T) })
    }

    /// The first entity carrying `T`, borrowed in place.
    ///
    /// Most control state is a singleton — one camera, one run, one cursor —
    /// and this is that case without the query dance around it.
    pub fn first_mut<T: Schema>(&mut self) -> Option<&mut T> {
        let e = self.first_entity::<T>()?;
        self.get_mut::<T>(e)
    }

    /// The entity id of the first thing carrying `T`.
    pub fn first_entity<T: Schema>(&mut self) -> Option<Entity> {
        let mut one = [0u64; 1];
        if self.query(T::NAME, &mut one) == 0 {
            None
        } else {
            Some(one[0])
        }
    }

    /// Every entity carrying `T`, into a caller-provided buffer. Returns the
    /// true count, which may exceed the buffer.
    pub fn entities_of<T: Schema>(&mut self, out: &mut [Entity]) -> u32 {
        self.query(T::NAME, out)
    }

    /// A copy of `T`, by type rather than by name.
    pub fn read<T: Schema>(&mut self, e: Entity) -> Option<T> {
        self.get::<T>(e, T::NAME)
    }

    /// Write a whole component, by type rather than by name.
    pub fn write<T: Schema>(&mut self, e: Entity, v: &T) -> bool {
        self.set(e, T::NAME, v)
    }

    pub fn remove(&mut self, e: Entity, name: &'static str) -> bool {
        self.raw_remove(e, Str::new(name))
    }
    pub fn query(&mut self, name: &'static str, out: &mut [Entity]) -> u32 {
        unsafe { self.raw_query(Str::new(name), out.as_mut_ptr(), out.len() as u32) }
    }
    pub fn asset(&mut self, name: &str) -> Option<&'static [u8]> {
        let mut s = Slice::<u8>::empty();
        let name = Str { ptr: name.as_ptr(), len: name.len() };
        if unsafe { self.raw_asset(name, &mut s) } {
            Some(unsafe { s.as_slice() })
        } else {
            None
        }
    }
    pub fn set_slot(&mut self, slot: Slot, module: &'static str) {
        self.raw_set_slot(slot, Str::new(module))
    }
    pub fn swipe(&mut self, dir: i32) {
        self.raw_swipe(dir)
    }
    pub fn quit(&mut self) {
        self.raw_quit()
    }
    pub fn log(&mut self, msg: &str) {
        self.raw_log(Str { ptr: msg.as_ptr(), len: msg.len() })
    }

    /// Ask for the data to be written out under `name`.
    ///
    /// design.md gives control the save: *"game.so 可以從data status s/l 去載入
    /// 存檔遊戲進度"*. Like every other request, it is collected now and
    /// applied at the frame boundary — a tick that snapshotted halfway through
    /// itself would be saving a state no frame ever had.
    pub fn save(&mut self, name: &str) -> bool {
        self.raw_save(Str { ptr: name.as_ptr(), len: name.len() })
    }

    /// Ask for `name` to be read back in.
    ///
    /// What returns is *status*. The roster is reconciled afterwards, because
    /// what should exist is control's answer and not the file's — a save taken
    /// against four definitions loads against five and gains the fifth.
    pub fn load(&mut self, name: &str) -> bool {
        self.raw_load(Str { ptr: name.as_ptr(), len: name.len() })
    }
}

/// `start`, `tick` and `stop` are pointers into the *module's own* table,
/// which is right on both routes: the host calls them through the module —
/// by address on `.so`, through the exported table on wasm — rather than
/// dereferencing them as its own.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ControlSpec {
    pub name: Str,
    /// Which module fills which slot when this control takes over.
    pub slots: Slice<SlotBind>,
    /// Called once before the first tick, after slots are bound.
    pub start: Option<unsafe extern "C" fn(*mut Ctl)>,
    pub tick: unsafe extern "C" fn(*mut Ctl, *const Frame),
    /// Called before this control is unloaded, including on hot swap.
    pub stop: Option<unsafe extern "C" fn(*mut Ctl)>,
}
