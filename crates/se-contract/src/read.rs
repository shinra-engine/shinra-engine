//! Reading the contract table back, the way the host will.
//!
//! Nothing here casts a byte slice to a struct. The table is a run of
//! little-endian `u32`s and every accessor reads four bytes at a time, so the
//! reader has no alignment requirement and cannot be wrong about padding —
//! which matters because the bytes arrive from a file, at whatever address a
//! `Vec<u8>` happened to land on.

use crate::layout::Ty;
use crate::table::{
    slot, ty_of_code, CONTROL_BYTES, FIELD_BYTES, IFACE_COMPONENTS, IFACE_CONTROL, MAGIC,
    POOL_BYTES, RECORD_BYTES, VERSION,
};
use anyhow::{bail, Result};

pub struct Table<'a> {
    bytes: &'a [u8],
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Ty,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordInfo {
    pub name: String,
    pub iface: u32,
    pub size: u32,
    pub align: u32,
    pub fields: Vec<FieldInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PoolInfo {
    pub name: String,
    /// Column indices, which are record indices: components come first.
    pub columns: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlInfo {
    pub name: String,
    pub at: u32,
    pub len: u32,
}

impl<'a> Table<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Table<'a>> {
        if bytes.len() < slot::COUNT * 4 {
            bail!("contract table is {} bytes, too short for its header", bytes.len())
        }
        if &bytes[..4] != MAGIC {
            bail!("not a contract table")
        }
        let t = Table { bytes };
        if t.slot(slot::VERSION) != VERSION {
            bail!(
                "contract table is version {}, this engine reads {VERSION}",
                t.slot(slot::VERSION)
            )
        }
        Ok(t)
    }

    fn u32_at(&self, at: usize) -> u32 {
        let b = &self.bytes[at..at + 4];
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }

    pub fn slot(&self, i: usize) -> u32 {
        self.u32_at(i * 4)
    }

    fn text(&self, off: u32, len: u32) -> Result<String> {
        let base = self.slot(slot::OFF_STRINGS) as usize + off as usize;
        let end = base + len as usize;
        if end > self.bytes.len() {
            bail!("a name runs past the end of the table")
        }
        Ok(String::from_utf8(self.bytes[base..end].to_vec())?)
    }

    pub fn arena_base(&self) -> u32 { self.slot(slot::ARENA_BASE) }
    pub fn arena_frame(&self) -> u32 { self.slot(slot::ARENA_FRAME) }
    pub fn arena_commands(&self) -> u32 { self.slot(slot::ARENA_COMMANDS) }
    pub fn arena_pools(&self) -> u32 { self.slot(slot::ARENA_POOLS) }
    pub fn arena_columns(&self) -> u32 { self.slot(slot::ARENA_COLUMNS) }
    pub fn arena_blobs(&self) -> u32 { self.slot(slot::ARENA_BLOBS) }
    pub fn arena_heap(&self) -> u32 { self.slot(slot::ARENA_HEAP) }
    pub fn max_commands(&self) -> u32 { self.slot(slot::MAX_COMMANDS) }
    pub fn blob_slots(&self) -> u32 { self.slot(slot::BLOB_SLOTS) }

    pub fn records(&self) -> Result<Vec<RecordInfo>> {
        let n = self.slot(slot::N_RECORDS) as usize;
        let base = self.slot(slot::OFF_RECORDS) as usize;
        let fbase = self.slot(slot::OFF_FIELDS) as usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let r = base + i * RECORD_BYTES;
            let name = self.text(self.u32_at(r), self.u32_at(r + 4))?;
            let first = self.u32_at(r + 20) as usize;
            let nf = self.u32_at(r + 24) as usize;
            let mut fields = Vec::with_capacity(nf);
            for k in 0..nf {
                let f = fbase + (first + k) * FIELD_BYTES;
                let fname = self.text(self.u32_at(f), self.u32_at(f + 4))?;
                let code = self.u32_at(f + 8);
                let Some(ty) = ty_of_code(code) else {
                    bail!("`{name}.{fname}` has unknown type code {code}")
                };
                fields.push(FieldInfo { name: fname, ty, offset: self.u32_at(f + 12) });
            }
            out.push(RecordInfo {
                name,
                iface: self.u32_at(r + 8),
                size: self.u32_at(r + 12),
                align: self.u32_at(r + 16),
                fields,
            });
        }
        Ok(out)
    }

    /// The components, which are the first `n` records and therefore indexed
    /// by column index.
    pub fn components(&self) -> Result<Vec<RecordInfo>> {
        Ok(self.records()?.into_iter().filter(|r| r.iface == IFACE_COMPONENTS).collect())
    }

    pub fn control_records(&self) -> Result<Vec<RecordInfo>> {
        Ok(self.records()?.into_iter().filter(|r| r.iface == IFACE_CONTROL).collect())
    }

    pub fn pools(&self) -> Result<Vec<PoolInfo>> {
        let n = self.slot(slot::N_POOLS) as usize;
        let base = self.slot(slot::OFF_POOLS) as usize;
        let cbase = self.slot(slot::OFF_POOL_COLS) as usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let p = base + i * POOL_BYTES;
            let name = self.text(self.u32_at(p), self.u32_at(p + 4))?;
            let first = self.u32_at(p + 8) as usize;
            let nc = self.u32_at(p + 12) as usize;
            let columns = (0..nc).map(|k| self.u32_at(cbase + (first + k) * 4)).collect();
            out.push(PoolInfo { name, columns });
        }
        Ok(out)
    }

    /// Where each control var sits, so a host — or an IDE — can show the
    /// control var set by name without compiling the game's contract in.
    pub fn control(&self) -> Result<Vec<ControlInfo>> {
        let n = self.slot(slot::N_CONTROL) as usize;
        let base = self.slot(slot::OFF_CONTROL) as usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let c = base + i * CONTROL_BYTES;
            out.push(ControlInfo {
                name: self.text(self.u32_at(c), self.u32_at(c + 4))?,
                at: self.u32_at(c + 8),
                len: self.u32_at(c + 12),
            });
        }
        Ok(out)
    }
}
