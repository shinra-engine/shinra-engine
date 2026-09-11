//! WGSL's layout rules, which are the contract's layout rules.
//!
//! design.md: *由GPU影響CPU排佈*. The direction matters. WIT says *what* a
//! field is and has neither fixed-size arrays nor any way to place one; WGSL
//! says *where* it sits. So this module is the authority on offsets, and the
//! Rust emitter's job is to insert padding until `#[repr(C)]` agrees with it.
//!
//! Everything here is std430 (the storage address space). The one asymmetry
//! that makes the whole exercise necessary is `vec3`: WGSL aligns it to 16
//! while its size stays 12, so a `[f32; 3]` — align 4 — lands somewhere else
//! the moment anything follows it.

/// A field, once WIT's naming has been resolved to something with a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    F32,
    I32,
    U32,
    /// `vec2`/`vec3`/`vec4` of `f32`.
    Vec(u32),
}

impl Ty {
    pub const fn align(self) -> u32 {
        match self {
            Ty::F32 | Ty::I32 | Ty::U32 => 4,
            Ty::Vec(2) => 8,
            // vec3 and vec4 both align to 16. vec3's size does not follow.
            Ty::Vec(_) => 16,
        }
    }

    pub const fn size(self) -> u32 {
        match self {
            Ty::F32 | Ty::I32 | Ty::U32 => 4,
            Ty::Vec(n) => 4 * n,
        }
    }

    pub fn rust(self) -> String {
        match self {
            Ty::F32 => "f32".into(),
            Ty::I32 => "i32".into(),
            Ty::U32 => "u32".into(),
            Ty::Vec(n) => format!("[f32; {n}]"),
        }
    }

    pub fn wgsl(self) -> String {
        match self {
            Ty::F32 => "f32".into(),
            Ty::I32 => "i32".into(),
            Ty::U32 => "u32".into(),
            Ty::Vec(n) => format!("vec{n}<f32>"),
        }
    }

    /// Components, for a vertex attribute.
    pub const fn lanes(self) -> u32 {
        match self {
            Ty::Vec(n) => n,
            _ => 1,
        }
    }
}

pub const fn round_up(align: u32, n: u32) -> u32 {
    n.div_ceil(align) * align
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub ty: Ty,
    /// std430. The number `#[repr(C)]` will be padded into agreeing with.
    pub offset: u32,
}

#[derive(Clone, Debug)]
pub struct Record {
    pub name: String,
    /// The WIT interface it came from, which decides what may be done with it.
    pub iface: String,
    pub fields: Vec<Field>,
    pub size: u32,
    pub align: u32,
}

/// Lay a record out: each member at the next offset its own alignment allows,
/// the record aligned to its widest member, its size rounded up to that.
pub fn lay_out(name: &str, iface: &str, fields: &[(String, Ty)]) -> Record {
    let mut out = Vec::with_capacity(fields.len());
    let mut cursor = 0u32;
    let mut align = 1u32;
    for (fname, ty) in fields {
        let offset = round_up(ty.align(), cursor);
        cursor = offset + ty.size();
        align = align.max(ty.align());
        out.push(Field { name: fname.clone(), ty: *ty, offset });
    }
    Record {
        name: name.to_string(),
        iface: iface.to_string(),
        fields: out,
        size: round_up(align, cursor),
        align,
    }
}

impl Record {
    /// Where a gap sits and how wide, walking the fields in order. The Rust
    /// emitter turns each into an explicit `_pad` member; nothing else needs
    /// to know they exist.
    pub fn gaps(&self) -> Vec<(usize, u32)> {
        let mut out = Vec::new();
        let mut cursor = 0u32;
        for (i, f) in self.fields.iter().enumerate() {
            if f.offset > cursor {
                out.push((i, f.offset - cursor));
            }
            cursor = f.offset + f.ty.size();
        }
        if self.size > cursor {
            out.push((self.fields.len(), self.size - cursor));
        }
        out
    }
}
