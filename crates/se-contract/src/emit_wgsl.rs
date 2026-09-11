//! `contract.wgsl` — the same records, as the shader sees them.
//!
//! Only `components` and `control` cross into a shader. `handles` and `render`
//! are plumbing between the host and a module, and a shader that could name
//! them would only be able to misuse them.
//!
//! The reserved-word gate is the useful part. A field name comes from
//! contract.wit, so a collision produces a shader that will not parse and an
//! error pointing at generated source the author never wrote. Refusing early,
//! by name, is far kinder — `se-render/src/attrs.rs` refuses the same way, and
//! this list is its list.

use crate::parse::Contract;
use crate::{snake, upper_camel};
use anyhow::{bail, Result};
use std::fmt::Write;

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

pub fn emit(c: &Contract) -> Result<String> {
    let shader_side: Vec<_> = c
        .records
        .iter()
        .filter(|r| r.iface == "components" || r.iface == "control")
        .collect();

    for r in &shader_side {
        for f in &r.fields {
            let n = snake(&f.name);
            if RESERVED.contains(&n.as_str()) {
                bail!(
                    "`{}.{}` cannot cross into a shader: `{n}` is a WGSL reserved word. \
                     Rename the field in contract.wit.",
                    r.name,
                    f.name
                );
            }
            if n.starts_with("se_") || n.starts_with("__") {
                bail!(
                    "`{}.{}` uses a reserved prefix; `se_` and `__` belong to the engine",
                    r.name,
                    f.name
                );
            }
        }
    }

    let mut out = String::from(
        "// generated from contract.wit by `se-contract` — do not edit\n\
         //\n\
         // Only `components` and `control` appear here. The offsets are the same\n\
         // std430 offsets the Rust records were padded into, so a column is instance\n\
         // data as it stands.\n\n",
    );
    for r in &shader_side {
        let _ = writeln!(out, "struct {} {{", upper_camel(&r.name));
        for f in &r.fields {
            let _ = writeln!(
                out,
                "    @align({}) {}: {},",
                f.ty.align(),
                snake(&f.name),
                f.ty.wgsl()
            );
        }
        out.push_str("};\n\n");
    }

    out.push_str("// --- instance inputs: the same records, with locations ---\n\n");
    for r in c.components() {
        let _ = writeln!(out, "struct {}Instance {{", upper_camel(&r.name));
        for (i, f) in r.fields.iter().enumerate() {
            let _ = writeln!(out, "    @location({i}) {}: {},", snake(&f.name), f.ty.wgsl());
        }
        out.push_str("};\n\n");
    }
    Ok(out)
}
