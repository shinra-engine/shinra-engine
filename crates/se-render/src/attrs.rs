//! Turning a component layout into shader inputs.
//!
//! The host has no Rust type for `Transform` — only the layout `data.rs`
//! registered. That turns out to be enough to generate both the vertex
//! attribute table and the WGSL struct that receives it, so a shader's
//! instance input is derived from the component rather than restated beside
//! it. Add a field to `data.rs` and it appears in the shader.

use anyhow::{bail, Result};
use se_abi::ScalarTy;
use se_host::registry::LayoutDef;
use std::fmt::Write;

/// First free `@location` — 0..2 belong to the mesh vertex.
pub const INSTANCE_BASE: u32 = 3;

/// WGSL keywords and reserved words that a component field may not be named.
///
/// Field names come straight from `data.rs`, so a collision produces a shader
/// that will not parse — and the resulting error points at generated source
/// the author never wrote. Refusing early, by name, is far kinder.
const RESERVED: &[&str] = &[
    "alias", "break", "case", "const", "const_assert", "continue", "continuing",
    "default", "diagnostic", "discard", "else", "enable", "false", "fn", "for",
    "if", "let", "loop", "override", "requires", "return", "struct", "switch",
    "true", "var", "while",
    // Reserved for future use by the WGSL spec.
    "as", "asm", "do", "enum", "extends", "filter", "goto", "handle", "impl",
    "in", "inline", "macro", "match", "mut", "of", "out", "package", "priv",
    "pub", "ref", "regardless", "static", "super", "target", "template",
    "this", "trait", "try", "type", "typedef", "union", "unless", "using",
    "virtual", "where", "yield",
    // Predeclared types and constructors that shadowing would break.
    "array", "atomic", "bool", "f16", "f32", "i32", "mat2x2", "mat3x3",
    "mat4x4", "ptr", "sampler", "texture_2d", "u32", "vec2", "vec3", "vec4",
];

fn check_name(component: &str, field: &str) -> Result<()> {
    if RESERVED.contains(&field) {
        bail!(
            "`{component}.{field}` cannot cross into a shader: `{field}` is a WGSL reserved word. \
             Rename the field in bundle/data.rs."
        );
    }
    if field.starts_with("se_") || field.starts_with("__") {
        bail!("`{component}.{field}` uses a reserved prefix; `se_` and `__` belong to the engine");
    }
    Ok(())
}

fn wgsl_scalar(t: ScalarTy) -> Result<&'static str> {
    Ok(match t {
        ScalarTy::F32 => "f32",
        ScalarTy::I32 => "i32",
        ScalarTy::U32 => "u32",
        ScalarTy::U8 | ScalarTy::F64 => {
            bail!("`{}` cannot be a shader input; use f32/i32/u32", t.name())
        }
    })
}

fn vertex_format(t: ScalarTy, n: u32) -> Result<wgpu::VertexFormat> {
    use wgpu::VertexFormat as F;
    Ok(match (t, n) {
        (ScalarTy::F32, 1) => F::Float32,
        (ScalarTy::F32, 2) => F::Float32x2,
        (ScalarTy::F32, 3) => F::Float32x3,
        (ScalarTy::F32, 4) => F::Float32x4,
        (ScalarTy::I32, 1) => F::Sint32,
        (ScalarTy::I32, 2) => F::Sint32x2,
        (ScalarTy::I32, 3) => F::Sint32x3,
        (ScalarTy::I32, 4) => F::Sint32x4,
        (ScalarTy::U32, 1) => F::Uint32,
        (ScalarTy::U32, 2) => F::Uint32x2,
        (ScalarTy::U32, 3) => F::Uint32x3,
        (ScalarTy::U32, 4) => F::Uint32x4,
        _ => bail!("no vertex format for {} x{n}", t.name()),
    })
}

fn wgsl_vec(t: ScalarTy, n: u32) -> Result<String> {
    let s = wgsl_scalar(t)?;
    Ok(if n == 1 { s.to_string() } else { format!("vec{n}<{s}>") })
}

/// The instance side of a draw: `wgpu` attributes and the matching WGSL.
pub struct Instancing {
    pub attributes: Vec<wgpu::VertexAttribute>,
    pub stride: u64,
    pub wgsl: String,
}

/// A field wider than four elements — a matrix, say — becomes consecutive
/// locations, because a vertex attribute tops out at four components.
pub fn instancing(layout: &LayoutDef) -> Result<Instancing> {
    let mut attributes = Vec::new();
    let mut wgsl = String::from("struct SeInstance {\n");
    let mut loc = INSTANCE_BASE;

    for f in &layout.fields {
        check_name(&layout.name, &f.name)?;
        let mut left = f.count.max(1);
        let mut off = f.offset;
        let mut part = 0;
        while left > 0 {
            let n = left.min(4);
            attributes.push(wgpu::VertexAttribute {
                format: vertex_format(f.ty, n)?,
                offset: off as u64,
                shader_location: loc,
            });
            let name = if f.count > 4 {
                format!("{}_{part}", f.name)
            } else {
                f.name.clone()
            };
            let _ = writeln!(wgsl, "    @location({loc}) {name} : {},", wgsl_vec(f.ty, n)?);
            off += n * f.ty.size();
            left -= n;
            loc += 1;
            part += 1;
        }
    }
    wgsl.push_str("};\n");

    let stride = {
        let align = layout.align.max(1);
        (layout.size.div_ceil(align) * align) as u64
    };
    Ok(Instancing { attributes, stride, wgsl })
}

/// Uniforms are the one place the Rust layout cannot be handed over as-is:
/// WGSL aligns `vec3` to 16 bytes and `#[repr(C)]` aligns it to 4. So every
/// field is repacked into whole `vec4` slots — predictable on both sides, at
/// the cost of writing `u.eye.xyz` in the shader.
pub struct Uniform {
    /// Bytes per field-slot group; always a multiple of 16.
    pub size: usize,
    pub wgsl: String,
    slots: Vec<(u32, u32, ScalarTy, u32)>,
}

pub fn uniform(layout: &LayoutDef) -> Result<Uniform> {
    let mut wgsl = String::from("struct SeUniform {\n");
    let mut slots = Vec::new();
    let mut out_off = 0u32;

    for f in &layout.fields {
        check_name(&layout.name, &f.name)?;
        let scalar = wgsl_scalar(f.ty)?;
        let mut left = f.count.max(1);
        let mut src = f.offset;
        let mut part = 0;
        while left > 0 {
            let n = left.min(4);
            let name = if f.count > 4 {
                format!("{}_{part}", f.name)
            } else {
                f.name.clone()
            };
            let _ = writeln!(wgsl, "    {name} : vec4<{scalar}>,");
            slots.push((src, out_off, f.ty, n));
            src += n * f.ty.size();
            out_off += 16;
            left -= n;
            part += 1;
        }
    }
    wgsl.push_str("};\n");
    Ok(Uniform { size: out_off.max(16) as usize, wgsl, slots })
}

impl Uniform {
    /// Copy one component's bytes into the padded uniform layout.
    pub fn pack(&self, src: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; self.size];
        for &(from, to, ty, n) in &self.slots {
            let bytes = (n * ty.size()) as usize;
            let (a, b) = (from as usize, to as usize);
            if a + bytes <= src.len() && b + bytes <= out.len() {
                out[b..b + bytes].copy_from_slice(&src[a..a + bytes]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
