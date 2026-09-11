//! wasm32 offsets for the ABI structs the host reads back.
//!
//! Each `OFF` block is the field layout `rustc` gives that struct on wasm32,
//! where a pointer is four bytes. They are constants rather than arithmetic so
//! that a mistake is visible in one place, and `tests/parity.rs` proves each
//! one by comparing against the same source built as a `.so`.

use crate::mem::{Mem, Registered};
use anyhow::Result;
use se_abi::{Extent, Format, ScalarTy};
use se_host::registry::{BufferDef, FieldDef, LayoutDef};

fn scalar(v: u32) -> Result<ScalarTy> {
    Ok(match v {
        0 => ScalarTy::U8,
        1 => ScalarTy::I32,
        2 => ScalarTy::U32,
        3 => ScalarTy::F32,
        4 => ScalarTy::F64,
        _ => anyhow::bail!("{v} is not a ScalarTy"),
    })
}

fn extent(v: u32) -> Result<Extent> {
    Ok(match v {
        0 => Extent::Screen,
        1 => Extent::Fixed,
        _ => anyhow::bail!("{v} is not an Extent"),
    })
}

/// `Field { name: Str, ty: u32, offset: u32, count: u32 }`
mod field {
    pub const NAME: u32 = 0;
    pub const TY: u32 = 8;
    pub const OFFSET: u32 = 12;
    pub const COUNT: u32 = 16;
    pub const SIZE: u32 = 20;
}

/// `Layout { name: Str, size: u32, align: u32, fields: Slice, hash: u64 }`
///
/// `hash` is a `u64`, which wasm32 aligns to 8 — so the struct is 8-aligned
/// and `fields` ends exactly on that boundary with no padding needed.
mod layout {
    pub const NAME: u32 = 0;
    pub const SIZE_F: u32 = 8;
    pub const ALIGN_F: u32 = 12;
    pub const FIELDS: u32 = 16;
    pub const HASH: u32 = 24;
    pub const SIZE: u32 = 32;
}

/// `BufferDesc { name: Str, extent: u32, width: u32, height: u32,
///               format: u32, count: u32, sampled: bool }`
mod buffer {
    pub const NAME: u32 = 0;
    pub const EXTENT: u32 = 8;
    pub const WIDTH: u32 = 12;
    pub const HEIGHT: u32 = 16;
    pub const FORMAT: u32 = 20;
    pub const COUNT: u32 = 24;
    pub const SAMPLED: u32 = 28;
    pub const SIZE: u32 = 32;
}

pub fn layouts(m: &Mem, r: &Registered) -> Result<Vec<LayoutDef>> {
    let mut out = Vec::with_capacity(r.len as usize);
    for i in 0..r.len {
        let at = r.elem(i, layout::SIZE, "se_register_layouts")?;
        let (fp, fn_) = m.slice(at + layout::FIELDS)?;
        let mut fields = Vec::with_capacity(fn_ as usize);
        for k in 0..fn_ {
            let f = fp + k * field::SIZE;
            fields.push(FieldDef {
                name: m.str(f + field::NAME)?,
                ty: scalar(m.u32(f + field::TY)?)?,
                offset: m.u32(f + field::OFFSET)?,
                count: m.u32(f + field::COUNT)?,
            });
        }
        out.push(LayoutDef {
            name: m.str(at + layout::NAME)?,
            size: m.u32(at + layout::SIZE_F)?,
            align: m.u32(at + layout::ALIGN_F)?,
            fields,
            hash: m.u64(at + layout::HASH)?,
        });
    }
    Ok(out)
}

pub fn buffers(m: &Mem, r: &Registered) -> Result<Vec<BufferDef>> {
    let mut out = Vec::with_capacity(r.len as usize);
    for i in 0..r.len {
        let at = r.elem(i, buffer::SIZE, "se_register_buffers")?;
        out.push(BufferDef {
            name: m.str(at + buffer::NAME)?,
            extent: extent(m.u32(at + buffer::EXTENT)?)?,
            width: m.u32(at + buffer::WIDTH)?,
            height: m.u32(at + buffer::HEIGHT)?,
            format: format(m.u32(at + buffer::FORMAT)?)?,
            count: m.u32(at + buffer::COUNT)?,
            sampled: m.bool(at + buffer::SAMPLED)?,
        });
    }
    Ok(out)
}

fn format(v: u32) -> Result<Format> {
    // Kept exhaustive by hand rather than transmuted: a module built against a
    // newer `se` would otherwise turn an unknown format into a valid-looking
    // one.
    Ok(match v {
        0 => Format::Rgba8Unorm,
        1 => Format::Rgba8UnormSrgb,
        2 => Format::Rgba16Float,
        3 => Format::R32Float,
        4 => Format::Depth32Float,
        _ => anyhow::bail!("{v} is not a Format"),
    })
}
