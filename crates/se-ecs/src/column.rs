//! Dense component bytes plus the sparse index that finds them.
//!
//! Split from the world proper because a column is the part that answers the
//! host's [`ColumnRef`] contract: it is what a stage signature resolves to and
//! what an instance buffer is filled from. The world above it only decides
//! which columns exist and which entities are in them.

use se_abi::Entity;
use se_host::registry::LayoutDef;
use se_host::space::ColumnRef;

use crate::{index_of, ABSENT};


/// Dense component bytes plus the sparse index that finds them.
pub struct Column {
    pub layout: LayoutDef,
    /// `size` rounded up to `align`, so element `i` starts at `i * stride`.
    pub stride: u32,
    /// Backed by `u64` so the base is 8-aligned. Every `ScalarTy` has
    /// alignment at most 8, so a `#[repr(C)]` component built from them can
    /// never need more — the alignment is correct by construction, not luck.
    data: Vec<u64>,
    rows: usize,
    dense_of: Vec<Entity>,
    sparse: Vec<u32>,
}

impl Column {
    pub(crate) fn new(layout: LayoutDef) -> Column {
        let align = layout.align.max(1);
        let stride = layout.size.div_ceil(align) * align;
        Column {
            layout,
            stride: stride.max(1),
            data: Vec::new(),
            rows: 0,
            dense_of: Vec::new(),
            sparse: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.rows
    }
    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }
    pub fn entities(&self) -> &[Entity] {
        &self.dense_of
    }

    /// The dense rows as raw bytes — what gets uploaded as instance data.
    /// Includes inter-element padding, which is exactly what the GPU stride
    /// expects.
    pub fn bytes(&self) -> &[u8] {
        let n = self.rows * self.stride as usize;
        let base = self.data.as_ptr() as *const u8;
        // SAFETY: `reserve_rows` allocated at least this many bytes.
        unsafe { std::slice::from_raw_parts(base, n) }
    }

    /// One dense row, or `None` when the column is shorter than that.
    pub fn row(&self, i: usize) -> Option<&[u8]> {
        (i < self.rows).then(|| self.slot(i as u32))
    }

    pub fn base_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr() as *mut u8
    }

    /// Dense row holding `e`, if any.
    pub fn row_of(&self, e: Entity) -> Option<u32> {
        let i = index_of(e) as usize;
        let d = *self.sparse.get(i)?;
        if d == ABSENT {
            None
        } else {
            Some(d)
        }
    }

    pub(crate) fn reserve_rows(&mut self, rows: usize) {
        let need_bytes = rows * self.stride as usize;
        let need_words = need_bytes.div_ceil(8);
        if self.data.len() < need_words {
            self.data.resize(need_words, 0);
        }
    }

    pub(crate) fn slot_mut(&mut self, row: u32) -> &mut [u8] {
        let off = row as usize * self.stride as usize;
        let n = self.layout.size as usize;
        let base = self.data.as_mut_ptr() as *mut u8;
        // SAFETY: `reserve_rows` guaranteed the words behind `row` exist.
        unsafe { std::slice::from_raw_parts_mut(base.add(off), n) }
    }

    pub(crate) fn slot(&self, row: u32) -> &[u8] {
        let off = row as usize * self.stride as usize;
        let n = self.layout.size as usize;
        let base = self.data.as_ptr() as *const u8;
        unsafe { std::slice::from_raw_parts(base.add(off), n) }
    }

    pub(crate) fn insert(&mut self, e: Entity, bytes: &[u8]) {
        let i = index_of(e) as usize;
        if self.sparse.len() <= i {
            self.sparse.resize(i + 1, ABSENT);
        }
        let row = if self.sparse[i] == ABSENT {
            let row = self.rows as u32;
            self.rows += 1;
            self.reserve_rows(self.rows);
            self.dense_of.push(e);
            self.sparse[i] = row;
            row
        } else {
            self.sparse[i]
        };
        let n = self.layout.size as usize;
        self.slot_mut(row)[..n].copy_from_slice(&bytes[..n]);
    }

    /// Swap-remove, so the dense array stays packed.
    pub(crate) fn remove(&mut self, e: Entity) -> bool {
        let i = index_of(e) as usize;
        let Some(&row) = self.sparse.get(i) else { return false };
        if row == ABSENT {
            return false;
        }
        let last = (self.rows - 1) as u32;
        if row != last {
            let stride = self.stride as usize;
            let base = self.data.as_mut_ptr() as *mut u8;
            // SAFETY: both rows are in range and do not overlap.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    base.add(last as usize * stride),
                    base.add(row as usize * stride),
                    self.layout.size as usize,
                );
            }
            let moved = self.dense_of[last as usize];
            self.dense_of[row as usize] = moved;
            self.sparse[index_of(moved) as usize] = row;
        }
        self.dense_of.pop();
        self.sparse[i] = ABSENT;
        self.rows -= 1;
        true
    }
}

/// A sparse-set column, seen as a plain run of records.
impl ColumnRef for Column {
    fn len(&self) -> usize {
        Column::len(self)
    }
    fn stride(&self) -> u32 {
        self.stride
    }
    fn bytes(&self) -> &[u8] {
        Column::bytes(self)
    }
    fn entities(&self) -> &[Entity] {
        Column::entities(self)
    }
    fn row_of(&self, e: Entity) -> Option<u32> {
        Column::row_of(self, e)
    }
}
