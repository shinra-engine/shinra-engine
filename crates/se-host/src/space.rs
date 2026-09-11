//! The space: named regions of memory, and their shape.
//!
//! This is the whole of what the render side needs from the data — and
//! deliberately much less than an ECS. A pass draws `Transform` by reading a
//! dense run of records with a known layout; it never asks which entity a
//! record belongs to, never queries, and has no notion of a component.
//!
//! Keeping that distinction sharp is what makes `design.md`'s claim — *"ecs只是
//! 其中一種實現"* — true rather than aspirational. Entities, components and
//! queries belong to the pipeline; the *space* belongs to the bundle, and any
//! pipeline that fills it the same way can be drawn by the same render module.
//! A pipeline built on CUDA arrays instead of sparse sets would present this
//! same interface and need no change on the render side at all.
//!
//! It is also the seam that survived the jump to wasm, and the only part of
//! this module that did. `ColumnRef` and `Store` are gone with the ECS:
//! design.md keeps only `process.wasm`, so there is no entity to key a column
//! by and no query to resolve. `bytes` is a slice rather than a raw pointer,
//! which is why it still means the same thing now that the memory it names is
//! a linear memory every module shares.

use crate::registry::LayoutDef;

/// A dense run of records of one kind.
pub struct Records<'a> {
    /// The records back to back, including any inter-record padding — which is
    /// exactly what a GPU instance buffer expects.
    pub bytes: &'a [u8],
    /// Distance between records. `size` rounded up to `align`.
    pub stride: u32,
    pub len: usize,
}

impl<'a> Records<'a> {
    /// One record, or `None` past the end.
    pub fn row(&self, i: usize) -> Option<&'a [u8]> {
        if i >= self.len {
            return None;
        }
        let off = i * self.stride as usize;
        self.bytes.get(off..off + self.stride as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// What the render side is allowed to know about the data.
///
/// Read-only by construction: rendering is the pure half of the design, and an
/// interface that cannot write is a cheaper guarantee than one that promises
/// not to.
pub trait Space {
    /// The shape of records under `name`, or `None` if the bundle declares no
    /// such space.
    fn layout(&self, name: &str) -> Option<&LayoutDef>;

    /// The records currently stored under `name`.
    fn records(&self, name: &str) -> Option<Records<'_>>;
}


