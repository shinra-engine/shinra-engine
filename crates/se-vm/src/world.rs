//! The world: columns placed in the arena, and the descriptors that say where.
//!
//! Nothing here is an ECS. There is no entity, no sparse set and no query —
//! design.md keeps only `process.wasm`, so a column is a dense run of records
//! and row `i` is row `i` in every column of its pool. What used to be a query
//! is now the pool: the columns that share one length.
//!
//! Every address past the tables is chosen here and published, never baked. A
//! module reads `column.base` each frame, which is what lets the host grow or
//! move a column with nothing rebuilt.

use crate::mem::Mem;
use crate::memory;
use anyhow::{bail, Context, Result};
use se_contract::read::Table;

/// `handles::column` as this engine requires it. The contract declares it, so
/// the declaration is checked rather than assumed — a game that spelled it
/// differently would otherwise be read at the wrong offsets.
const COLUMN_FIELDS: [(&str, u32); 4] =
    [("base", 0), ("cap", 4), ("stride", 8), ("pool", 12)];
const POOL_FIELDS: [(&str, u32); 2] = [("len", 0), ("cap", 4)];
const SPAN_FIELDS: [(&str, u32); 2] = [("off", 0), ("len", 4)];

pub const COLUMN_BYTES: u32 = 16;
pub const POOL_BYTES: u32 = 8;
pub const SPAN_BYTES: u32 = 8;

fn check_handle(t: &Table, name: &str, want: &[(&str, u32)], size: u32) -> Result<()> {
    let r = t
        .records()?
        .into_iter()
        .find(|r| r.name == name)
        .with_context(|| format!("the contract declares no `handles.{name}`"))?;
    if r.size != size {
        bail!("`handles.{name}` is {} bytes; this engine reads {size}", r.size)
    }
    for (fname, at) in want {
        let f = r
            .fields
            .iter()
            .find(|f| f.name == *fname)
            .with_context(|| format!("`handles.{name}` has no `{fname}`"))?;
        if f.offset != *at {
            bail!("`handles.{name}.{fname}` is at {}; this engine reads {at}", f.offset)
        }
    }
    Ok(())
}

/// The contract, decoded once into something owned.
///
/// The table borrows the bytes it was read from, which would tie every call
/// that also needs `&mut Store` in a knot for no gain: the contract does not
/// change while a game runs, so it is decoded at load and kept.
pub struct Meta {
    pub base: u32,
    pub frame: u32,
    pub commands: u32,
    pub pools_at: u32,
    pub columns_at: u32,
    pub blobs_at: u32,
    pub heap: u32,
    pub max_commands: u32,
    pub blob_slots: u32,
    pub components: Vec<se_contract::read::RecordInfo>,
    /// Every record, so the engine can check the ones it defines itself.
    pub all: Vec<se_contract::read::RecordInfo>,
    pub pools: Vec<se_contract::read::PoolInfo>,
    pub control: Vec<se_contract::read::ControlInfo>,
}

impl Meta {
    pub fn read(t: &Table) -> Result<Meta> {
        memory::check_base(t.arena_base())?;
        check_handle(t, "column", &COLUMN_FIELDS, COLUMN_BYTES)?;
        check_handle(t, "pool", &POOL_FIELDS, POOL_BYTES)?;
        check_handle(t, "span", &SPAN_FIELDS, SPAN_BYTES)?;
        Ok(Meta {
            base: t.arena_base(),
            frame: t.arena_frame(),
            commands: t.arena_commands(),
            pools_at: t.arena_pools(),
            columns_at: t.arena_columns(),
            blobs_at: t.arena_blobs(),
            heap: t.arena_heap(),
            max_commands: t.max_commands(),
            blob_slots: t.blob_slots(),
            components: t.components()?,
            all: t.records()?,
            pools: t.pools()?,
            control: t.control()?,
        })
    }

    /// Where a control var sits, by the name contract.wit gave it.
    pub fn control_at(&self, name: &str) -> Option<(u32, u32)> {
        self.control.iter().find(|c| c.name == name).map(|c| (c.at, c.len))
    }

    /// Any record the contract declares, by name — including the ones no
    /// shader sees, which is how the GPU side checks `render::command`.
    pub fn record(&self, name: &str) -> Option<&se_contract::read::RecordInfo> {
        self.all.iter().find(|r| r.name == name)
    }

    pub fn column_index(&self, name: &str) -> Option<u32> {
        self.components.iter().position(|r| r.name == name).map(|i| i as u32)
    }

    /// Which pool a column belongs to. A column in no pool has no length, so
    /// this is an error rather than an `Option`: `se-contract` refuses such a
    /// contract, and a table that still contains one is corrupt.
    pub fn pool_of(&self, column: u32) -> Result<u32> {
        for (i, p) in self.pools.iter().enumerate() {
            if p.columns.contains(&column) {
                return Ok(i as u32);
            }
        }
        bail!(
            "column {column} (`{}`) is in no pool, so it has no length",
            self.components
                .get(column as usize)
                .map(|r| r.name.as_str())
                .unwrap_or("?")
        )
    }

    pub fn pool_index(&self, name: &str) -> Option<u32> {
        self.pools.iter().position(|p| p.name == name).map(|i| i as u32)
    }
}

pub struct World {
    /// One length per pool, mirrored from the arena so the host need not read
    /// it back to make a decision.
    pub pool_len: Vec<u32>,
    pub pool_cap: Vec<u32>,
    /// Per column: where it sits and how wide a row is.
    pub base: Vec<u32>,
    pub stride: Vec<u32>,
    /// Which pool each column belongs to.
    pub pool_of: Vec<u32>,
    /// First free byte after the columns: blob payloads go here.
    pub blob_top: u32,
    /// Per blob slot: how many bytes its current region can hold, so a
    /// re-put that fits rewrites in place instead of growing `blob_top`.
    pub blob_cap: Vec<u32>,
}

fn w32(mem: &mut dyn Mem, at: u32, v: u32) -> Result<()> {
    mem.write(at, &v.to_le_bytes())
}

fn r32(mem: &dyn Mem, at: u32) -> Result<u32> {
    let b = mem.read(at, 4)?;
    Ok(u32::from_le_bytes(b.try_into().expect("read returns the length asked")))
}

impl World {
    /// Place every column and publish the descriptors.
    ///
    /// `cap` is rows per pool. One number for now: a pool is homogeneous by
    /// construction, so its columns cannot want different capacities.
    pub fn place(m: &Meta, mem: &mut dyn Mem, cap: u32) -> Result<World> {
        let comps = &m.components;
        let pools = &m.pools;
        if pools.is_empty() {
            bail!("the contract declares no pool, so no column has a length")
        }

        let mut pool_of = vec![u32::MAX; comps.len()];
        for (pi, p) in pools.iter().enumerate() {
            for c in &p.columns {
                let Some(slot) = pool_of.get_mut(*c as usize) else {
                    bail!("pool `{}` names column {c}, which does not exist", p.name)
                };
                *slot = pi as u32;
            }
        }

        // Size the placement before writing anything, so a world that will not
        // fit is refused rather than half-built. In u64: `size * cap` is
        // caller-chosen and must not wrap past the check below.
        let mut bytes = Vec::with_capacity(comps.len());
        let mut sum = 0u64;
        for r in comps {
            let b = r.size as u64 * cap as u64;
            sum += b;
            if sum > u32::MAX as u64 {
                bail!(
                    "a capacity of {cap} rows does not fit: column `{}` ({} bytes a row) \
                     takes the world past 4 GiB",
                    r.name,
                    r.size
                )
            }
            bytes.push(b as u32);
        }
        let total = sum as u32;
        memory::check_heap(m.heap, total)?;

        let mut base = Vec::with_capacity(comps.len());
        let mut stride = Vec::with_capacity(comps.len());
        let mut at = m.heap;
        for (i, r) in comps.iter().enumerate() {
            base.push(at);
            stride.push(r.size);
            let d = m.columns_at + i as u32 * COLUMN_BYTES;
            w32(mem, d, at)?;
            w32(mem, d + 4, cap)?;
            w32(mem, d + 8, r.size)?;
            w32(mem, d + 12, pool_of[i])?;
            at += bytes[i];
        }
        for pi in 0..pools.len() as u32 {
            let d = m.pools_at + pi * POOL_BYTES;
            w32(mem, d, 0)?;
            w32(mem, d + 4, cap)?;
        }

        Ok(World {
            pool_len: vec![0; pools.len()],
            pool_cap: vec![cap; pools.len()],
            base,
            stride,
            pool_of,
            blob_top: at,
            blob_cap: vec![0; m.blob_slots as usize],
        })
    }

    /// Set a pool's length — the only thing that changes how many rows exist.
    /// A control var asks for it; nothing spawns.
    pub fn set_len(&mut self, m: &Meta, mem: &mut dyn Mem, pool: u32, len: u32) -> Result<()> {
        let i = pool as usize;
        let Some(cap) = self.pool_cap.get(i).copied() else {
            bail!("no pool {pool}")
        };
        let len = len.min(cap);
        w32(mem, m.pools_at + pool * POOL_BYTES, len)?;
        self.pool_len[i] = len;
        Ok(())
    }

    pub fn len_of(&self, m: &Meta, mem: &dyn Mem, pool: u32) -> Result<u32> {
        r32(mem, m.pools_at + pool * POOL_BYTES)
    }

    /// Copy bytes into a blob region and publish the span. Variable length
    /// lives here; the record that names it stays the size it was.
    pub fn put_blob(
        &mut self,
        m: &Meta,
        mem: &mut dyn Mem,
        slot: u32,
        payload: &[u8],
    ) -> Result<(u32, u32)> {
        if slot >= m.blob_slots {
            bail!("blob slot {slot} is past the {} the contract declares", m.blob_slots)
        }
        let len = payload.len() as u32;
        let d = m.blobs_at + slot * SPAN_BYTES;
        let old = r32(mem, d)?;
        let at = if len <= self.blob_cap[slot as usize] {
            old
        } else {
            let at = (self.blob_top + 15) & !15;
            memory::check_heap(at, len)?;
            self.blob_top = at + len;
            self.blob_cap[slot as usize] = len;
            at
        };
        mem.write(at, payload)?;
        w32(mem, d, at)?;
        w32(mem, d + 4, len)?;
        Ok((at, len))
    }

    pub fn row_at(&self, column: u32, row: u32) -> u32 {
        self.base[column as usize] + row * self.stride[column as usize]
    }
}
