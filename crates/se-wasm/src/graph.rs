//! wasm32 offsets for the graph and stage descriptions.
//!
//! A graph is pure data — names, shader source, clear colours — so it crosses
//! backends unchanged and decodes into the host's own `GraphDef`.
//!
//! A stage does not, quite. `StageSpec` ends in a function pointer, which on
//! wasm is a table index and not something the host can hold, so only a
//! stage's *description* is decoded here. That is what `schedule::validate`
//! needs, and it is what makes a wasm bundle checkable before anything runs.

use crate::mem::{Mem, Registered};
use anyhow::{bail, Result};
use se_abi::{Access, DrawKind, ParamKind};
use se_host::registry::{DrawDef, GraphDef, ParamDef, PassDef};

/// `Draw { kind: u32, instance_of: Str, mesh: Str }`
mod draw {
    pub const KIND: u32 = 0;
    pub const INSTANCE_OF: u32 = 4;
    pub const MESH: u32 = 12;
}

/// `Pass { name, shader, color, depth, reads, textures, uniform_of,
///         clear: [f32; 4], load: bool, draw: Draw }`
mod pass {
    pub const NAME: u32 = 0;
    pub const SHADER: u32 = 8;
    pub const COLOR: u32 = 16;
    pub const DEPTH: u32 = 24;
    pub const READS: u32 = 32;
    pub const TEXTURES: u32 = 40;
    pub const UNIFORM_OF: u32 = 48;
    pub const CLEAR: u32 = 56;
    pub const LOAD: u32 = 72;
    pub const DRAW: u32 = 76;
    pub const SIZE: u32 = 96;
}

mod edge {
    pub const FROM: u32 = 0;
    pub const TO: u32 = 8;
    pub const SIZE: u32 = 16;
}

mod graph {
    pub const NAME: u32 = 0;
    pub const PASSES: u32 = 8;
    pub const EDGES: u32 = 16;
    pub const PRESENT: u32 = 24;
    pub const SIZE: u32 = 32;
}

/// `Param { name: Str, kind: u32, access: u32, size: u32, hash: u64 }`
///
/// `hash` is a `u64` and wasm32 aligns those to 8, so the four bytes after
/// `size` are padding and the struct is 32 wide, not 28.
mod param {
    pub const NAME: u32 = 0;
    pub const KIND: u32 = 8;
    pub const ACCESS: u32 = 12;
    pub const SIZE_F: u32 = 16;
    pub const HASH: u32 = 24;
    pub const SIZE: u32 = 32;
}

/// `StageSpec { name: Str, params: Slice<Param>, run: fn }` — the function
/// pointer is a four-byte table index here.
mod spec {
    pub const NAME: u32 = 0;
    pub const PARAMS: u32 = 8;
    #[allow(dead_code)]
    pub const SIZE: u32 = 20;
}

/// A stage as the wasm backend sees it: everything but how to call it.
#[derive(Debug, Clone)]
pub struct StageInfo {
    pub name: String,
    pub params: Vec<ParamDef>,
}

fn param_kind(v: u32) -> Result<ParamKind> {
    Ok(match v {
        0 => ParamKind::Component,
        1 => ParamKind::Resource,
        2 => ParamKind::Dt,
        _ => bail!("{v} is not a ParamKind"),
    })
}

fn access(v: u32) -> Result<Access> {
    Ok(match v {
        0 => Access::Read,
        1 => Access::Write,
        _ => bail!("{v} is not an Access"),
    })
}

fn draw_kind(v: u32) -> Result<DrawKind> {
    Ok(match v {
        0 => DrawKind::Fullscreen,
        1 => DrawKind::Instanced,
        _ => bail!("{v} is not a DrawKind"),
    })
}

fn strs(m: &Mem, at: u32) -> Result<Vec<String>> {
    let (p, n) = m.slice(at)?;
    (0..n).map(|i| m.str(p + i * 8)).collect()
}

pub fn stage_at(m: &Mem, at: u32) -> Result<StageInfo> {
    let (pp, pn) = m.slice(at + spec::PARAMS)?;
    let mut params = Vec::with_capacity(pn as usize);
    for i in 0..pn {
        let p = pp + i * param::SIZE;
        params.push(ParamDef {
            name: m.str(p + param::NAME)?,
            kind: param_kind(m.u32(p + param::KIND)?)?,
            access: access(m.u32(p + param::ACCESS)?)?,
            size: m.u32(p + param::SIZE_F)?,
            hash: m.u64(p + param::HASH)?,
        });
    }
    Ok(StageInfo { name: m.str(at + spec::NAME)?, params })
}

pub fn graphs(m: &Mem, r: &Registered) -> Result<Vec<GraphDef>> {
    let mut out = Vec::with_capacity(r.len as usize);
    for i in 0..r.len {
        let at = r.elem(i, graph::SIZE, "se_register_graph")?;
        let (pp, pn) = m.slice(at + graph::PASSES)?;
        let mut passes = Vec::with_capacity(pn as usize);
        for k in 0..pn {
            let p = pp + k * pass::SIZE;
            let d = p + pass::DRAW;
            let instance_of = m.str(d + draw::INSTANCE_OF)?;
            let mesh = m.str(d + draw::MESH)?;
            let mut clear = [0.0f32; 4];
            for (j, c) in clear.iter_mut().enumerate() {
                *c = f32::from_bits(m.u32(p + pass::CLEAR + j as u32 * 4)?);
            }
            passes.push(PassDef {
                name: m.str(p + pass::NAME)?,
                shader: m.str(p + pass::SHADER)?,
                color: strs(m, p + pass::COLOR)?,
                depth: Some(m.str(p + pass::DEPTH)?).filter(|s| !s.is_empty()),
                reads: strs(m, p + pass::READS)?,
                textures: strs(m, p + pass::TEXTURES)?,
                uniform_of: Some(m.str(p + pass::UNIFORM_OF)?).filter(|s| !s.is_empty()),
                clear,
                load: m.bool(p + pass::LOAD)?,
                draw: DrawDef {
                    kind: draw_kind(m.u32(d + draw::KIND)?)?,
                    instance_of: Some(instance_of).filter(|s| !s.is_empty()),
                    mesh: Some(mesh).filter(|s| !s.is_empty()),
                },
            });
        }
        let (ep, en) = m.slice(at + graph::EDGES)?;
        let mut edges = Vec::with_capacity(en as usize);
        for k in 0..en {
            let e = ep + k * edge::SIZE;
            edges.push((m.str(e + edge::FROM)?, m.str(e + edge::TO)?));
        }
        out.push(GraphDef {
            name: m.str(at + graph::NAME)?,
            passes,
            edges,
            present: m.str(at + graph::PRESENT)?,
        });
    }
    Ok(out)
}
