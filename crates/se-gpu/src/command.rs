//! The command list `render.wasm` records, read back out of the arena.
//!
//! `render.wasm` does not call WebGPU. It writes a run of fixed-size commands
//! and the host replays them — one boundary crossing a frame instead of one
//! per draw, and the module keeps the zero-import property the shared-stack
//! memory model depends on.
//!
//! Fixed size is the point: the host walks the list with a stride and needs no
//! decoder. The layout is `render::command` in contract.wit, so the offsets
//! below are checked against the contract at load rather than assumed.

use anyhow::{bail, Result};
use se_vm::world::Meta;

pub const BEGIN_PASS: u32 = 0;
pub const DRAW_INSTANCED: u32 = 1;
pub const DRAW_FULLSCREEN: u32 = 2;
pub const END_PASS: u32 = 3;

/// `render::command`, as this engine reads it.
const COMMAND_FIELDS: [(&str, u32); 12] = [
    ("kind", 0),
    ("vertices", 4),
    ("shader", 8),
    ("column", 12),
    ("count", 16),
    ("uniform_off", 20),
    ("uniform_len", 24),
    ("pad", 28),
    ("clear_r", 32),
    ("clear_g", 36),
    ("clear_b", 40),
    ("clear_a", 44),
];
const FRAME_FIELDS: [(&str, u32); 4] =
    [("commands", 0), ("count", 4), ("present", 8), ("epoch", 12)];

pub const COMMAND_BYTES: u32 = 48;
pub const FRAME_BYTES: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Command {
    pub kind: u32,
    /// How many vertices the shader builds per instance. A render module knows
    /// its own geometry; the host does not, and should not have to guess.
    pub vertices: u32,
    pub shader: u32,
    pub column: u32,
    pub count: u32,
    pub uniform_off: u32,
    pub uniform_len: u32,
    pub clear: [f32; 4],
}

/// WIT spells names in kebab-case and the generator snakes them; compare on
/// the snaked form, which is what both emitters produced.
fn check(meta: &Meta, iface_name: &str, want: &[(&str, u32)], size: u32) -> Result<()> {
    let r = meta
        .record(iface_name)
        .ok_or_else(|| anyhow::anyhow!("the contract declares no `render.{iface_name}`"))?;
    if r.size != size {
        bail!("`render.{iface_name}` is {} bytes; this engine reads {size}", r.size)
    }
    for (name, at) in want {
        let f = r
            .fields
            .iter()
            .find(|f| f.name.replace('-', "_") == *name)
            .ok_or_else(|| anyhow::anyhow!("`render.{iface_name}` has no `{name}`"))?;
        if f.offset != *at {
            bail!("`render.{iface_name}.{name}` is at {}; this engine reads {at}", f.offset)
        }
    }
    Ok(())
}

pub fn check_layout(meta: &Meta) -> Result<()> {
    check(meta, "command", &COMMAND_FIELDS, COMMAND_BYTES)?;
    check(meta, "frame", &FRAME_FIELDS, FRAME_BYTES)?;
    Ok(())
}

fn u32_at(m: &[u8], at: u32) -> u32 {
    let a = at as usize;
    u32::from_le_bytes([m[a], m[a + 1], m[a + 2], m[a + 3]])
}

fn f32_at(m: &[u8], at: u32) -> f32 {
    f32::from_bits(u32_at(m, at))
}

/// The list the module wrote this frame, and the epoch it stamped.
pub fn read(meta: &Meta, mem: &[u8]) -> Result<(Vec<Command>, u32)> {
    let f = meta.frame;
    if (f + FRAME_BYTES) as usize > mem.len() {
        bail!("the frame header is outside the memory")
    }
    let at = u32_at(mem, f);
    let count = u32_at(mem, f + 4);
    let epoch = u32_at(mem, f + 12);
    if count > meta.max_commands {
        bail!("render.wasm recorded {count} commands; the contract allows {}", meta.max_commands)
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let c = at + i * COMMAND_BYTES;
        if (c + COMMAND_BYTES) as usize > mem.len() {
            bail!("command {i} is outside the memory")
        }
        out.push(Command {
            kind: u32_at(mem, c),
            vertices: u32_at(mem, c + 4),
            shader: u32_at(mem, c + 8),
            column: u32_at(mem, c + 12),
            count: u32_at(mem, c + 16),
            uniform_off: u32_at(mem, c + 20),
            uniform_len: u32_at(mem, c + 24),
            clear: [
                f32_at(mem, c + 32),
                f32_at(mem, c + 36),
                f32_at(mem, c + 40),
                f32_at(mem, c + 44),
            ],
        });
    }
    Ok((out, epoch))
}
