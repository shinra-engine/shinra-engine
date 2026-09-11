//! Component storage, owned by the host and outliving every module.
//!
//! One sparse set per component, keyed by name rather than by Rust type — the
//! host has no types, only layouts, which is exactly what lets `data.rs` be
//! recompiled underneath a running world.

use crate::registry::LayoutDef;
use crate::space::{Records, Space};
use se_abi::Entity;
use std::collections::HashMap;

const ABSENT: u32 = u32::MAX;

#[inline]
fn pack(index: u32, gen: u32) -> Entity {
    ((gen as u64) << 32) | index as u64
}
#[inline]
fn index_of(e: Entity) -> u32 {
    e as u32
}
#[inline]
fn gen_of(e: Entity) -> u32 {
    (e >> 32) as u32
}

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
    fn new(layout: LayoutDef) -> Column {
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

    fn reserve_rows(&mut self, rows: usize) {
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

    fn slot(&self, row: u32) -> &[u8] {
        let off = row as usize * self.stride as usize;
        let n = self.layout.size as usize;
        let base = self.data.as_ptr() as *const u8;
        unsafe { std::slice::from_raw_parts(base.add(off), n) }
    }

    fn insert(&mut self, e: Entity, bytes: &[u8]) {
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
    fn remove(&mut self, e: Entity) -> bool {
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

/// Everything that exists. Modules come and go; this does not.
pub struct World {
    cols: HashMap<String, Column>,
    /// Generation per entity index; odd means alive.
    gens: Vec<u32>,
    free: Vec<u32>,
    live: Vec<Entity>,
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

impl World {
    pub fn new() -> World {
        World { cols: HashMap::new(), gens: Vec::new(), free: Vec::new(), live: Vec::new() }
    }

    /// Install the bundle's layouts. Returns the components whose shape moved,
    /// which the caller must treat as a contract break.
    pub fn declare(&mut self, layouts: &[LayoutDef]) -> Vec<String> {
        let mut changed = Vec::new();
        for l in layouts {
            match self.cols.get(&l.name) {
                Some(c) if c.layout.hash == l.hash => {}
                Some(_) => changed.push(l.name.clone()),
                None => {
                    self.cols.insert(l.name.clone(), Column::new(l.clone()));
                }
            }
        }
        changed
    }

    pub fn layout(&self, name: &str) -> Option<&LayoutDef> {
        self.cols.get(name).map(|c| &c.layout)
    }
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.cols.get(name)
    }
    pub fn column_mut(&mut self, name: &str) -> Option<&mut Column> {
        self.cols.get_mut(name)
    }
    pub fn component_names(&self) -> impl Iterator<Item = &String> {
        self.cols.keys()
    }
    pub fn entities(&self) -> &[Entity] {
        &self.live
    }

    pub fn spawn(&mut self) -> Entity {
        let index = match self.free.pop() {
            Some(i) => i,
            None => {
                self.gens.push(0);
                (self.gens.len() - 1) as u32
            }
        };
        self.gens[index as usize] += 1;
        let e = pack(index, self.gens[index as usize]);
        self.live.push(e);
        e
    }

    pub fn alive(&self, e: Entity) -> bool {
        let i = index_of(e) as usize;
        self.gens.get(i).is_some_and(|&g| g == gen_of(e) && g % 2 == 1)
    }

    pub fn despawn(&mut self, e: Entity) -> bool {
        if !self.alive(e) {
            return false;
        }
        for c in self.cols.values_mut() {
            c.remove(e);
        }
        let i = index_of(e) as usize;
        self.gens[i] += 1;
        self.free.push(i as u32);
        if let Some(p) = self.live.iter().position(|&x| x == e) {
            self.live.swap_remove(p);
        }
        true
    }

    /// Write a component. Refused when the size disagrees with the bundle
    /// layout — a stale module is a bug to report, not bytes to reinterpret.
    pub fn set(&mut self, e: Entity, name: &str, bytes: &[u8]) -> bool {
        if !self.alive(e) {
            return false;
        }
        let Some(c) = self.cols.get_mut(name) else { return false };
        if bytes.len() != c.layout.size as usize {
            return false;
        }
        c.insert(e, bytes);
        true
    }

    pub fn get(&self, e: Entity, name: &str) -> Option<&[u8]> {
        if !self.alive(e) {
            return None;
        }
        let c = self.cols.get(name)?;
        c.row_of(e).map(|r| c.slot(r))
    }

    /// A component borrowed in place, for a caller that will write to it.
    ///
    /// The pointer is only good until the next structural change: an insert
    /// can grow the column and move it. Callers above hold `&mut Ctl` for the
    /// life of the borrow, which is what makes that impossible rather than
    /// merely discouraged.
    pub fn get_mut(&mut self, e: Entity, name: &str) -> Option<&mut [u8]> {
        if !self.alive(e) {
            return None;
        }
        let c = self.cols.get_mut(name)?;
        let row = c.row_of(e)?;
        Some(c.slot_mut(row))
    }

    pub fn remove(&mut self, e: Entity, name: &str) -> bool {
        self.cols.get_mut(name).is_some_and(|c| c.remove(e))
    }
}

/// The sparse-set world, seen as plain space.
///
/// Nothing here mentions an entity: that is the point. When this storage moves
/// out of the host and into a pipeline module, this is the interface the
/// render side keeps talking to, unchanged.
impl Space for World {
    fn layout(&self, name: &str) -> Option<&LayoutDef> {
        World::layout(self, name)
    }

    fn records(&self, name: &str) -> Option<Records<'_>> {
        let c = self.column(name)?;
        Some(Records { bytes: c.bytes(), stride: c.stride, len: c.len() })
    }
}
