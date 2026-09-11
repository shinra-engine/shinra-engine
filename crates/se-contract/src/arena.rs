//! Where everything sits in the one linear memory every module imports.
//!
//! Generated rather than written down, because the placement is a consequence
//! of the contract and three module sources plus the host must not each
//! restate it.
//!
//! # The one invariant that will bite
//!
//! `BASE` must equal the `-z stack-size` every module is linked with.
//! Measured, not assumed: `--global-base` moves a module's *static data* and
//! does not move its stack, and rustc always passes `--stack-first`, so every
//! module's stack occupies `[0, stack-size)` — the same region in all of them.
//! Anything the host places below that is inside the stack and is destroyed
//! once a call nests deep enough. A world placed correctly instead sees a
//! stack overflow trap, which is the failure you want.
//!
//! That the region is shared at all is only safe because no two modules are
//! ever on the stack at once, which is what the zero-import rule buys: a
//! module that cannot call out cannot be suspended inside another one.

use crate::layout::round_up;
use crate::parse::Contract;

/// Uniform stack size, and therefore the arena's base. 64 KiB.
pub const STACK_SIZE: u32 = 0x0001_0000;
/// How many commands `render.wasm` may record in one frame.
pub const MAX_COMMANDS: u32 = 256;
/// Named blob slots — mesh and texture payloads an asset published.
pub const BLOB_SLOTS: u32 = 64;

#[derive(Clone, Debug)]
pub struct Region {
    pub name: String,
    pub at: u32,
    pub len: u32,
}

#[derive(Debug)]
pub struct Map {
    pub base: u32,
    /// One per `interface control` record.
    pub control: Vec<Region>,
    pub frame: u32,
    pub commands: u32,
    pub pools: u32,
    pub columns: u32,
    pub blobs: u32,
    /// Host-managed from here: column storage and blob payloads. Nothing baked.
    pub heap: u32,
}

fn sizeof(c: &Contract, iface: &str, name: &str) -> u32 {
    c.records
        .iter()
        .find(|r| r.iface == iface && r.name == name)
        .map(|r| r.size)
        .unwrap_or(0)
}

pub fn map(c: &Contract) -> Map {
    let base = STACK_SIZE;
    let mut at = base;

    // The control var set first: it is the smallest, the most read, and the
    // thing a debugger wants at a predictable address.
    let mut control = Vec::new();
    let mut names: Vec<&crate::layout::Record> = c.of("control").collect();
    names.sort_by(|a, b| a.name.cmp(&b.name));
    for r in names {
        at = round_up(16, at);
        control.push(Region { name: r.name.clone(), at, len: r.size });
        at += r.size;
    }

    at = round_up(64, at);
    let frame = at;
    at += sizeof(c, "render", "frame");

    at = round_up(64, at);
    let commands = at;
    at += sizeof(c, "render", "command") * MAX_COMMANDS;

    at = round_up(64, at);
    let pools = at;
    at += sizeof(c, "handles", "pool") * c.pools.len() as u32;

    at = round_up(64, at);
    let columns = at;
    at += sizeof(c, "handles", "column") * c.components().len() as u32;

    at = round_up(64, at);
    let blobs = at;
    at += sizeof(c, "handles", "span") * BLOB_SLOTS;

    let heap = round_up(4096, at);
    Map { base, control, frame, commands, pools, columns, blobs, heap }
}
