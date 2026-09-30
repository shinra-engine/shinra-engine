//! The contract as bytes the host reads at run time.
//!
//! `contract.rs` is for *modules*: they are compiled per game anyway, so they
//! may as well have the records as Rust types. The host may not. An engine
//! binary that compiled one game's contract in could only ever run that game,
//! and design.md wants the opposite — *像是短影音左滑一下就換了一款遊戲*.
//!
//! So the same contract is also emitted as a flat table, carried in a
//! `se.contract` custom section of `contract.wasm`. A wasm module with no
//! functions is still a wasm module: the host reads the section without
//! instantiating anything, and every wasm tool can still open the file.
//!
//! Every field is a little-endian `u32`. No pointers, no `usize`, no
//! alignment assumptions — the reader never casts, it reads.

use crate::arena::{Map, BLOB_SLOTS, MAX_COMMANDS};
use crate::layout::Ty;
use crate::parse::Contract;

pub const MAGIC: &[u8; 4] = b"SHNR";
pub const VERSION: u32 = 1;
pub const SECTION: &str = "se.contract";

/// Byte widths of the fixed-size rows. The encoder appends bytes and the
/// reader indexes by these, so they are named once rather than divided by a
/// literal in seven places.
pub const RECORD_BYTES: usize = 7 * 4;
pub const FIELD_BYTES: usize = 4 * 4;
pub const POOL_BYTES: usize = 4 * 4;
pub const CONTROL_BYTES: usize = 4 * 4;

/// Which interface a record came from, and therefore what may write it.
pub const IFACE_COMPONENTS: u32 = 0;
pub const IFACE_CONTROL: u32 = 1;
pub const IFACE_HANDLES: u32 = 2;
pub const IFACE_RENDER: u32 = 3;

pub fn iface_code(name: &str) -> u32 {
    match name {
        "components" => IFACE_COMPONENTS,
        "control" => IFACE_CONTROL,
        "handles" => IFACE_HANDLES,
        _ => IFACE_RENDER,
    }
}

pub const fn ty_code(t: Ty) -> u32 {
    match t {
        Ty::F32 => 0,
        Ty::I32 => 1,
        Ty::U32 => 2,
        Ty::Span => 3,
        Ty::Vec(n) => 2 + n,
    }
}

pub fn ty_of_code(c: u32) -> Option<Ty> {
    Some(match c {
        0 => Ty::F32,
        1 => Ty::I32,
        2 => Ty::U32,
        3 => Ty::Span,
        4..=6 => Ty::Vec(c - 2),
        _ => return None,
    })
}

struct Strings {
    bytes: Vec<u8>,
}

impl Strings {
    /// Interned, so a name repeated across records costs its bytes once.
    fn put(&mut self, s: &str) -> (u32, u32) {
        if let Some(at) = find(&self.bytes, s.as_bytes()) {
            return (at as u32, s.len() as u32);
        }
        let at = self.bytes.len() as u32;
        self.bytes.extend_from_slice(s.as_bytes());
        (at, s.len() as u32)
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn u32s(v: &mut Vec<u8>, xs: &[u32]) {
    for x in xs {
        v.extend_from_slice(&x.to_le_bytes());
    }
}

/// The header, in `u32` slots. Kept as an explicit list so the reader and the
/// writer cannot drift: both index the same names.
pub mod slot {
    pub const VERSION: usize = 1;
    pub const N_RECORDS: usize = 2;
    pub const N_FIELDS: usize = 3;
    pub const N_POOLS: usize = 4;
    pub const N_POOL_COLS: usize = 5;
    pub const OFF_RECORDS: usize = 6;
    pub const OFF_FIELDS: usize = 7;
    pub const OFF_POOLS: usize = 8;
    pub const OFF_POOL_COLS: usize = 9;
    pub const OFF_STRINGS: usize = 10;
    pub const ARENA_BASE: usize = 11;
    pub const ARENA_FRAME: usize = 12;
    pub const ARENA_COMMANDS: usize = 13;
    pub const ARENA_POOLS: usize = 14;
    pub const ARENA_COLUMNS: usize = 15;
    pub const ARENA_BLOBS: usize = 16;
    pub const ARENA_HEAP: usize = 17;
    pub const MAX_COMMANDS: usize = 18;
    pub const BLOB_SLOTS: usize = 19;
    pub const N_CONTROL: usize = 20;
    pub const OFF_CONTROL: usize = 21;
    /// Header length, in `u32`s. The first slot is the magic.
    pub const COUNT: usize = 22;
}

/// Records are emitted in the order the host will index them: components
/// first, alphabetically, so a column index is the record index.
pub fn encode(c: &Contract, m: &Map) -> Vec<u8> {
    let mut strings = Strings { bytes: Vec::new() };
    let mut records = Vec::new();
    let mut fields = Vec::new();

    let components = c.components();
    let mut ordered: Vec<&crate::layout::Record> = components.clone();
    ordered.extend(c.records.iter().filter(|r| r.iface != "components"));

    for r in &ordered {
        let (noff, nlen) = strings.put(&r.name);
        let first_field = (fields.len() / FIELD_BYTES) as u32;
        for f in &r.fields {
            let (foff, flen) = strings.put(&f.name);
            u32s(&mut fields, &[foff, flen, ty_code(f.ty), f.offset]);
        }
        u32s(
            &mut records,
            &[
                noff,
                nlen,
                iface_code(&r.iface),
                r.size,
                r.align,
                first_field,
                r.fields.len() as u32,
            ],
        );
    }

    let mut pools = Vec::new();
    let mut pool_cols = Vec::new();
    for p in &c.pools {
        let (noff, nlen) = strings.put(&p.name);
        let first = (pool_cols.len() / 4) as u32;
        for col in &p.columns {
            let idx = components.iter().position(|r| &r.name == col).expect("checked") as u32;
            u32s(&mut pool_cols, &[idx]);
        }
        u32s(&mut pools, &[noff, nlen, first, p.columns.len() as u32]);
    }

    // Control vars: name, arena offset, size. The host needs these to hand a
    // debugger — or an IDE — a named view of the control var set.
    let mut control = Vec::new();
    for r in &m.control {
        let (noff, nlen) = strings.put(&r.name);
        u32s(&mut control, &[noff, nlen, r.at, r.len]);
    }

    let head = slot::COUNT * 4;
    let off_records = head as u32;
    let off_fields = off_records + records.len() as u32;
    let off_pools = off_fields + fields.len() as u32;
    let off_pool_cols = off_pools + pools.len() as u32;
    let off_control = off_pool_cols + pool_cols.len() as u32;
    let off_strings = off_control + control.len() as u32;

    let mut out = Vec::with_capacity(off_strings as usize + strings.bytes.len());
    out.extend_from_slice(MAGIC);
    let mut h = vec![0u32; slot::COUNT - 1];
    h[slot::VERSION - 1] = VERSION;
    h[slot::N_RECORDS - 1] = (records.len() / RECORD_BYTES) as u32;
    h[slot::N_FIELDS - 1] = (fields.len() / FIELD_BYTES) as u32;
    h[slot::N_POOLS - 1] = (pools.len() / POOL_BYTES) as u32;
    h[slot::N_POOL_COLS - 1] = (pool_cols.len() / 4) as u32;
    h[slot::OFF_RECORDS - 1] = off_records;
    h[slot::OFF_FIELDS - 1] = off_fields;
    h[slot::OFF_POOLS - 1] = off_pools;
    h[slot::OFF_POOL_COLS - 1] = off_pool_cols;
    h[slot::OFF_STRINGS - 1] = off_strings;
    h[slot::ARENA_BASE - 1] = m.base;
    h[slot::ARENA_FRAME - 1] = m.frame;
    h[slot::ARENA_COMMANDS - 1] = m.commands;
    h[slot::ARENA_POOLS - 1] = m.pools;
    h[slot::ARENA_COLUMNS - 1] = m.columns;
    h[slot::ARENA_BLOBS - 1] = m.blobs;
    h[slot::ARENA_HEAP - 1] = m.heap;
    h[slot::MAX_COMMANDS - 1] = MAX_COMMANDS;
    h[slot::BLOB_SLOTS - 1] = BLOB_SLOTS;
    h[slot::N_CONTROL - 1] = (control.len() / CONTROL_BYTES) as u32;
    h[slot::OFF_CONTROL - 1] = off_control;
    u32s(&mut out, &h);
    out.extend_from_slice(&records);
    out.extend_from_slice(&fields);
    out.extend_from_slice(&pools);
    out.extend_from_slice(&pool_cols);
    out.extend_from_slice(&control);
    out.extend_from_slice(&strings.bytes);
    out
}
