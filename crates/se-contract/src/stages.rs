//! Finding the stages a process module brought, and refusing the ones that do
//! not fit the contract.
//!
//! This is where *behaviour is open while types are closed* actually happens.
//! `contract.wit` fixes the set of component types; it says nothing about what
//! may be done to them. A process module declares its stages in a `se.stages`
//! custom section — data, read without instantiating — and each stage names
//! the columns it reads and writes. The host checks those names against the
//! contract and then calls the stage by export name.
//!
//! Nothing in the host knows any stage. A process module written years from
//! now loads into a host built today, so long as it only names columns the
//! contract declares. [`validate`] is the only thing standing between "open"
//! and "unsound", which is why it is stricter than it looks.
//!
//! # Why the entries are self-describing
//!
//! Each `#[se::stage]` contributes its own `#[link_section]` static and
//! wasm-ld concatenates them in an order it does not promise. So there is no
//! header and no offset table: an entry carries its own length, and the reader
//! walks. Concatenate the section in any order, or in several pieces, and it
//! still reads.
//!
//! ```text
//! entry   "SEST" | entry_len | version | kind | n_params | name_len | name…
//!         then per param:  access | group | name_len | name…
//! ```
//! `kind` is [`ROW`] (one tuple of columns per call) or [`PAIR`] (two tuples:
//! `group` 0 and 1). A row stage puts every param in group 0.
//! Every scalar is a little-endian `u32`; every name is padded to four bytes.
//! A param names a *component*, not a column index: which index a column gets
//! is the contract's business, and a module should not have to know it.

use anyhow::{bail, Context, Result};

pub const SECTION: &str = "se.stages";
pub const MAGIC: &[u8; 4] = b"SEST";
pub const VERSION: u32 = 2;
/// What `#[se::stage]` names the export. A stage is discovered by this prefix,
/// never listed by the host.
pub const RUN_PREFIX: &str = "se_stage_run__";

pub const READ: u32 = 0;
pub const WRITE: u32 = 1;

/// A stage over single rows (`#[se::stage]`).
pub const ROW: u32 = 0;
/// A stage over pairs of rows (`#[se::pair]`).
pub const PAIR: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub component: String,
    pub access: u32,
    /// Which tuple of a pair stage the param belongs to; 0 for a row stage.
    pub group: u32,
    /// Filled in by [`resolve`], once the contract has been consulted.
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub name: String,
    pub kind: u32,
    pub params: Vec<Param>,
    /// `se_stage_run__<name>`, resolved once at load.
    pub export: String,
}

pub const fn pad4(n: usize) -> usize {
    n.div_ceil(4) * 4
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    let s = b.get(at..at + 4).context("the stage table is truncated")?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn name_at(b: &[u8], at: usize, len: usize) -> Result<String> {
    Ok(std::str::from_utf8(b.get(at..at + len).context("a name runs past the table")?)?.to_string())
}

/// Walk the concatenated entries. A module with no section has no stages,
/// which is the right answer for a control module or an asset.
pub fn decode(section: &[u8]) -> Result<Vec<Stage>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < section.len() {
        if section.len() - at < 24 {
            bail!("a stage entry at {at} is too short for its header")
        }
        if &section[at..at + 4] != MAGIC {
            bail!("the stage entry at {at} does not begin with SEST")
        }
        let entry_len = u32_at(section, at + 4)? as usize;
        if entry_len < 24 || at + entry_len > section.len() {
            bail!("the stage entry at {at} claims {entry_len} bytes")
        }
        let version = u32_at(section, at + 8)?;
        if version != VERSION {
            bail!("stage table version {version}; this engine reads {VERSION} — rebuild the module")
        }
        let kind = u32_at(section, at + 12)?;
        if kind != ROW && kind != PAIR {
            bail!("a stage entry has kind {kind}")
        }
        let n_params = u32_at(section, at + 16)? as usize;
        let name_len = u32_at(section, at + 20)? as usize;
        let name = name_at(section, at + 24, name_len)?;

        let mut p = at + 24 + pad4(name_len);
        let mut params = Vec::with_capacity(n_params);
        for _ in 0..n_params {
            let access = u32_at(section, p)?;
            let group = u32_at(section, p + 4)?;
            let len = u32_at(section, p + 8)? as usize;
            let component = name_at(section, p + 12, len)?;
            params.push(Param { component, access, group, column: u32::MAX });
            p += 12 + pad4(len);
        }
        if p > at + entry_len {
            bail!("stage `{name}` declares more than its entry holds")
        }
        out.push(Stage { export: format!("{RUN_PREFIX}{name}"), name, kind, params });
        at += entry_len;
    }
    // Declaration order within one static is preserved, but the order the
    // linker concatenates statics in is not. Sorting by name makes the frame
    // reproducible; a pipeline that needs a particular order should say so in
    // a `process.ron`, not depend on link order.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Build one entry — the encoder the macro will use, kept beside the decoder
/// so the two cannot drift.
pub fn encode(name: &str, kind: u32, params: &[(&str, u32, u32)]) -> Vec<u8> {
    fn put(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn put_name(v: &mut Vec<u8>, s: &str) {
        v.extend_from_slice(s.as_bytes());
        v.resize(v.len() + (pad4(s.len()) - s.len()), 0);
    }
    let mut body = Vec::new();
    put(&mut body, VERSION);
    put(&mut body, kind);
    put(&mut body, params.len() as u32);
    put(&mut body, name.len() as u32);
    put_name(&mut body, name);
    for (comp, access, group) in params {
        put(&mut body, *access);
        put(&mut body, *group);
        put(&mut body, comp.len() as u32);
        put_name(&mut body, comp);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(MAGIC);
    put(&mut out, (body.len() + 8) as u32);
    out.extend_from_slice(&body);
    out
}
