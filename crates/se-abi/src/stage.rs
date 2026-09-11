//! `process/*.so`: a stage is a function whose parameter list *is* its query.
//!
//! The signature is the whole contract. There is no world handle, no commands
//! buffer, no escape hatch — reaching outside the parameters is not
//! expressible, so single-writer reasoning holds by construction.

use crate::prim::{Slice, Str};

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read = 0,
    Write = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// Per-entity storage. Contributes to the query.
    Component = 0,
    /// A single value for the whole frame — `&Input`, and nothing a stage may
    /// write.
    Resource = 1,
    /// The bare `f32` tail: seconds since the previous tick.
    Dt = 2,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Param {
    pub name: Str,
    pub kind: ParamKind,
    pub access: Access,
    /// Size the module compiled against. Checked against the bundle layout so
    /// a stale `process.so` is refused instead of reading garbage.
    pub size: u32,
    /// Layout hash for components, 0 otherwise.
    pub hash: u64,
}

impl Param {
    pub const fn component(name: &'static str, access: Access, size: u32, hash: u64) -> Param {
        Param { name: Str::new(name), kind: ParamKind::Component, access, size, hash }
    }
    pub const fn resource(name: &'static str, size: u32) -> Param {
        Param {
            name: Str::new(name),
            kind: ParamKind::Resource,
            access: Access::Read,
            size,
            hash: 0,
        }
    }
    /// A `&mut` binding in the signature is what makes a parameter a write.
    pub const fn write(mut self) -> Param {
        self.access = Access::Write;
        self
    }
    pub const fn dt() -> Param {
        Param {
            name: Str::new("dt"),
            kind: ParamKind::Dt,
            access: Access::Read,
            size: 4,
            hash: 0,
        }
    }
}

/// One component column, as the host exposes it for a single call.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Column {
    pub base: *mut u8,
    pub stride: u32,
}

/// A batched invocation. The host resolves the query once and hands the whole
/// match set over in one call — the per-entity loop runs *inside* the module,
/// so crossing the boundary costs one call per stage, not one per entity.
#[repr(C)]
pub struct StageCall {
    /// One per `ParamKind::Component`, in parameter order.
    pub cols: *const Column,
    pub n_cols: u32,
    /// `n_rows * n_cols` dense indices, row-major: `rows[r * n_cols + c]` is
    /// the index into column `c` for match `r`. Components live in
    /// independent sparse sets, so the indices differ per column.
    pub rows: *const u32,
    pub n_rows: u32,
    /// One per `ParamKind::Resource`, in parameter order.
    pub res: *const *const u8,
    pub n_res: u32,
    pub dt: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StageSpec {
    pub name: Str,
    pub params: Slice<Param>,
    pub run: unsafe extern "C" fn(*const StageCall),
}


/// Index of parameter `i` *within its own kind* — its column index if it is a
/// component, its resource index if it is a resource. `#[se::stage]` folds
/// this at compile time so the shim indexes the call frame directly.
pub const fn slot_of(params: &[Param], i: usize) -> u32 {
    let kind = params[i].kind as u32;
    let mut n = 0u32;
    let mut j = 0usize;
    while j < i {
        if params[j].kind as u32 == kind {
            n += 1;
        }
        j += 1;
    }
    n
}
