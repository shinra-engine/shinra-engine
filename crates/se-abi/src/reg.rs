//! How a module hands its registrations back.
//!
//! The old shape was a sink: the host passed in a struct carrying one of its
//! own function pointers and the module called it once per item. That works
//! for a `.so`, where host and module share an address space, and cannot work
//! at all for wasm — inside a wasm module a host function pointer is a 32-bit
//! number naming nothing, and the module imports nothing that could stand in
//! for it. Registration would compile and be wrong at run time.
//!
//! So the direction is inverted. The module builds its own array, keeps it
//! alive for as long as its image, and returns where it is. The host reads it:
//!
//!   * `.so` — the pointer is an address, and the host dereferences it.
//!   * `.wasm` — the same integer is an offset into the module's exported
//!     linear memory, and the host reads the bytes out of there.
//!
//! One source, no `#[cfg]`, and a module that imports nothing at all. It also
//! costs the module nothing it was not already paying: `graph!` was already
//! building its description in full before handing it over item by item.

/// Where a module's registrations live, in that module's own address space.
///
/// Returned by pointer rather than by value on purpose. A multi-field return
/// goes through an out-pointer in the C ABI, and on wasm that pointer would
/// have to name memory *inside* the module — which the host cannot allocate
/// without the module exporting an allocator. A single pointer return is one
/// integer, and needs nothing.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Registered {
    /// Base of the array. Null when a module registers nothing.
    pub ptr: *const u8,
    pub len: u32,
    /// `size_of` one element. The host checks it against the type it is about
    /// to read, which is the cross-check the old function-pointer signature
    /// used to give for free.
    pub stride: u32,
}

impl Registered {
    pub const EMPTY: Registered = Registered { ptr: core::ptr::null(), len: 0, stride: 0 };

    /// # Safety
    /// `T` must be the element type this was built from, and the module must
    /// still be loaded.
    pub unsafe fn as_slice<T>(&self) -> &[T] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        core::slice::from_raw_parts(self.ptr as *const T, self.len as usize)
    }
}

/// A `Registered` parked in a module's static, so an entry point can build its
/// answer once and return the same pointer every time.
///
/// `Sync` by hand because it holds a raw pointer. What it points at is leaked
/// and never written again, so sharing it is sound — and the alternative,
/// rebuilding on every call, would hand the host a different pointer each time
/// and leak on every one.
#[repr(transparent)]
pub struct Held(pub Registered);

// SAFETY: the array is leaked at first use and immutable thereafter.
unsafe impl Sync for Held {}
unsafe impl Send for Held {}

impl Held {
    /// Describe a slice that will outlive the module's use of it. `se`'s
    /// entry macros leak a `Vec` into one; a module with constant data can
    /// hand over a `static` array directly.
    pub fn from_static<T>(items: &'static [T]) -> Held {
        Held(Registered {
            ptr: items.as_ptr() as *const u8,
            len: items.len() as u32,
            stride: core::mem::size_of::<T>() as u32,
        })
    }

    pub const fn as_ptr(&self) -> *const Registered {
        &self.0 as *const Registered
    }
}
