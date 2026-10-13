//! The save file: a header and the arena, nothing else.
//!
//! ```text
//! "SHSV"  version u32  contract-id u64  frame u32  base u32  len u32  | payload
//! ```
//!
//! Little-endian. The payload is `[meta.base, world.blob_top)` — control vars,
//! tables, column descriptors, column data and blob payloads.
//!
//! What a save does **not** hold: the wasm stack (`[0, STACK_SIZE)`) and every
//! module's statics (`Slot::global_base`). Swapping a module drops its statics
//! anyway, so state that must survive has to live in the arena.

use anyhow::{bail, Result};

pub const MAGIC: &[u8; 4] = b"SHSV";
pub const VERSION: u32 = 1;
pub const HEADER: usize = 4 + 4 + 8 + 4 + 4 + 4;

pub struct Header {
    pub id: u64,
    pub frame: u32,
    pub base: u32,
    pub len: u32,
}

pub use se_contract::table::fingerprint;

pub fn encode(id: u64, frame: u32, base: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(HEADER + payload.len());
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&VERSION.to_le_bytes());
    v.extend_from_slice(&id.to_le_bytes());
    v.extend_from_slice(&frame.to_le_bytes());
    v.extend_from_slice(&base.to_le_bytes());
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn decode(bytes: &[u8]) -> Result<(Header, &[u8])> {
    if bytes.len() < HEADER || &bytes[..4] != MAGIC {
        bail!("not a save file: bad magic")
    }
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let version = u32_at(4);
    if version != VERSION {
        bail!("save version {version} is not supported; this engine reads version {VERSION}")
    }
    let h = Header {
        id: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        frame: u32_at(16),
        base: u32_at(20),
        len: u32_at(24),
    };
    let payload = &bytes[HEADER..];
    if payload.len() != h.len as usize {
        bail!("save is truncated or padded: header says {} bytes, file holds {}", h.len, payload.len())
    }
    Ok((h, payload))
}
