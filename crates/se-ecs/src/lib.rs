//! A sparse-set ECS — one implementation of the host's data contract.
//!
//! This used to live inside `se-host`, which made the engine *have* an ECS.
//! It does not any more: the host holds a `dyn Store` and cannot name this
//! crate. That is what turns design.md's *"ecs只是其中一種實現 我們也可以使用
//! 不同方式實踐data update"* from a statement of intent into a property of the
//! build graph — a store built on archetypes, on flat arrays, or on CUDA
//! device memory drops in here with nothing above it recompiled.
//!
//! One sparse set per component, keyed by name rather than by Rust type — the
//! host has no types, only the layouts `bundle.so` declared, which is exactly
//! what lets `data.rs` be recompiled underneath a running store.

use se_abi::Entity;
use se_host::registry::LayoutDef;
use se_host::space::{ColumnRef, Records, Space, Store};
use std::collections::HashMap;

mod column;
pub use column::Column;

pub(crate) const ABSENT: u32 = u32::MAX;

#[inline]
pub(crate) fn pack(index: u32, gen: u32) -> Entity {
    ((gen as u64) << 32) | index as u64
}
#[inline]
pub(crate) fn index_of(e: Entity) -> u32 {
    e as u32
}
#[inline]
pub(crate) fn gen_of(e: Entity) -> u32 {
    (e >> 32) as u32
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

/// The read half: what the render side is allowed to know.
///
/// Nothing here mentions an entity, and that is the point — a pass asks for a
/// dense run of `Transform` records and gets one, with no way to discover that
/// a sparse set produced them.
impl Space for World {
    fn layout(&self, name: &str) -> Option<&LayoutDef> {
        World::layout(self, name)
    }

    fn records(&self, name: &str) -> Option<Records<'_>> {
        let c = World::column(self, name)?;
        Some(Records { bytes: c.bytes(), stride: c.stride, len: c.len() })
    }
}

/// The write half: what `game.so` and the scheduler drive.
///
/// Every method forwards to the inherent one above. The indirection is the
/// deliverable: it is the only thing standing between the host and this
/// crate's representation.
impl Store for World {
    fn declare(&mut self, layouts: &[LayoutDef]) -> Vec<String> {
        World::declare(self, layouts)
    }

    fn component_names(&self) -> Vec<&str> {
        self.cols.keys().map(|s| s.as_str()).collect()
    }

    fn entities(&self) -> &[Entity] {
        World::entities(self)
    }

    fn column(&self, name: &str) -> Option<&dyn ColumnRef> {
        World::column(self, name).map(|c| c as &dyn ColumnRef)
    }

    fn column_base(&mut self, name: &str) -> Option<(*mut u8, u32)> {
        let c = World::column_mut(self, name)?;
        Some((c.base_ptr(), c.stride))
    }

    fn spawn(&mut self) -> Entity {
        World::spawn(self)
    }
    fn despawn(&mut self, e: Entity) -> bool {
        World::despawn(self, e)
    }
    fn alive(&self, e: Entity) -> bool {
        World::alive(self, e)
    }
    fn set(&mut self, e: Entity, name: &str, bytes: &[u8]) -> bool {
        World::set(self, e, name, bytes)
    }
    fn get(&self, e: Entity, name: &str) -> Option<&[u8]> {
        World::get(self, e, name)
    }
    fn get_mut(&mut self, e: Entity, name: &str) -> Option<&mut [u8]> {
        World::get_mut(self, e, name)
    }
    fn remove(&mut self, e: Entity, name: &str) -> bool {
        World::remove(self, e, name)
    }
}
