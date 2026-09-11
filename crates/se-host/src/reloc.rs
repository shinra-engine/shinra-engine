//! Reading a module's registrations when it has its own linear memory.
//!
//! Both sides of the wasm route are `wasm32`, so every ABI struct has the same
//! size and the same field offsets in a module as in the engine. What differs
//! is only *where*: a pointer a module hands back is an offset into that
//! module's memory, not the engine's.
//!
//! So the engine copies the module's memory in once, at load, and reads the
//! structs as themselves — no field tables, no layout arithmetic — adding one
//! constant to every pointer it follows. That constant is the whole difference
//! between the two address spaces.
//!
//! Registration happens at load and not per frame, so the copy is paid once
//! per module. If it ever stops being worth it, design.md records the
//! zero-copy alternative: real PIC dynamic linking, where the module imports
//! the engine's memory and there is no delta at all.

use se_abi::{Registered, Slice, Str};

// The premise, checked rather than asserted in prose: this module reads a
// wasm32 module's structs as the engine's own, which is only sound because the
// engine is wasm32 too. If any of these ever disagree, the reader is silently
// reading the wrong bytes.
const _: () = {
    assert!(core::mem::size_of::<Registered>() == 12);
    assert!(core::mem::align_of::<Registered>() == 4);
    assert!(core::mem::size_of::<Str>() == 8);
    assert!(core::mem::size_of::<Slice<u8>>() == 8);
    assert!(core::mem::size_of::<*const u8>() == 4);
};

/// A module's memory, held by the engine, and the offset it now sits at.
pub struct Reloc {
    /// The module's memory, copied.
    pub mem: Vec<u8>,
}

impl Reloc {
    pub fn new(mem: Vec<u8>) -> Reloc {
        Reloc { mem }
    }

    fn at(&self, p: u32, len: usize) -> Option<&[u8]> {
        self.mem.get(p as usize..p as usize + len)
    }

    /// A struct the module placed at `p`, read as itself.
    ///
    /// # Safety
    /// `T` must be the ABI type the module wrote there, and `T` must be plain
    /// data — anything it points at needs [`Reloc::str`] or [`Reloc::slice`],
    /// because those pointers are still in the module's address space.
    pub unsafe fn get<T>(&self, p: u32) -> Option<&T> {
        let b = self.at(p, core::mem::size_of::<T>())?;
        if b.as_ptr() as usize % core::mem::align_of::<T>() != 0 {
            return None;
        }
        Some(&*(b.as_ptr() as *const T))
    }

    /// `n` of them, back to back.
    ///
    /// # Safety
    /// As [`Reloc::get`].
    pub unsafe fn array<T>(&self, p: u32, n: u32) -> Option<&[T]> {
        if n == 0 {
            return Some(&[]);
        }
        let b = self.at(p, core::mem::size_of::<T>() * n as usize)?;
        if b.as_ptr() as usize % core::mem::align_of::<T>() != 0 {
            return None;
        }
        Some(core::slice::from_raw_parts(b.as_ptr() as *const T, n as usize))
    }

    /// A `Str` whose pointer is a module-space offset.
    pub fn str(&self, s: &Str) -> Option<&str> {
        let b = self.at(s.ptr as u32, s.len)?;
        core::str::from_utf8(b).ok()
    }

    /// A `Slice<T>` whose pointer is a module-space offset.
    ///
    /// # Safety
    /// As [`Reloc::get`].
    pub unsafe fn slice<T>(&self, s: &Slice<T>) -> Option<&[T]> {
        self.array::<T>(s.ptr as u32, s.len as u32)
    }
}
