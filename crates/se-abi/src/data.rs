//! `bundle.so` half one: component layouts, from `data.rs`.

use crate::prim::{fnv1a, mix, Slice, Str};

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarTy {
    U8 = 0,
    I32 = 1,
    U32 = 2,
    F32 = 3,
    F64 = 4,
}

impl ScalarTy {
    pub const fn size(self) -> u32 {
        match self {
            ScalarTy::U8 => 1,
            ScalarTy::I32 | ScalarTy::U32 | ScalarTy::F32 => 4,
            ScalarTy::F64 => 8,
        }
    }
    pub const fn tag(self) -> u64 {
        self as u32 as u64
    }
    pub const fn name(self) -> &'static str {
        match self {
            ScalarTy::U8 => "u8",
            ScalarTy::I32 => "i32",
            ScalarTy::U32 => "u32",
            ScalarTy::F32 => "f32",
            ScalarTy::F64 => "f64",
        }
    }
}

/// One field of a component. `count` is 1 for a scalar, N for `[T; N]`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Field {
    pub name: Str,
    pub ty: ScalarTy,
    pub offset: u32,
    pub count: u32,
}

impl Field {
    pub const fn new(name: &'static str, ty: ScalarTy, offset: u32, count: u32) -> Field {
        Field { name: Str::new(name), ty, offset, count }
    }
}

/// The host never sees the Rust type — only this. Storage, save files and the
/// render side all work from the layout, which is what lets `data.rs` be
/// swapped without recompiling the host.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Layout {
    pub name: Str,
    pub size: u32,
    pub align: u32,
    pub fields: Slice<Field>,
    /// Structural hash over name + every field. Two layouts agreeing here are
    /// interchangeable; disagreeing means the bundle contract changed.
    pub hash: u64,
}

impl Layout {
    pub const fn new(
        name: &'static str,
        size: u32,
        align: u32,
        fields: &'static [Field],
        hash: u64,
    ) -> Layout {
        Layout {
            name: Str::new(name),
            size,
            align,
            fields: Slice::new(fields),
            hash,
        }
    }
}

/// The structural hash, folded one field at a time so `#[derive(Schema)]` can
/// build it in a `const` block while it still has the field-name literals in
/// hand. Host and module run the identical fold, so agreement is proof the two
/// were compiled against the same shape.
pub const fn hash_begin(name: &str, size: u32, align: u32) -> u64 {
    let mut h = fnv1a(name.as_bytes());
    h = mix(h, size as u64);
    mix(h, align as u64)
}

pub const fn hash_field(h: u64, fname: &str, ty: ScalarTy, offset: u32, count: u32) -> u64 {
    let mut h = mix(h, fnv1a(fname.as_bytes()));
    h = mix(h, ty.tag());
    h = mix(h, offset as u64);
    mix(h, count as u64)
}


/// Implemented by `#[derive(se::Schema)]`. The only way a `#[repr(C)]` struct
/// becomes a component the host can store.
pub trait Schema: Copy + 'static {
    const NAME: &'static str;
    const HASH: u64;
    fn layout() -> Layout;
}
