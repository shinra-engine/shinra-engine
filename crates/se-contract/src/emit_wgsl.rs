//! `contract.wgsl` — the same records, as the shader sees them.
//!
//! Only `components` and `control` cross into a shader. `handles` and `render`
//! are plumbing between the host and a module, and a shader that could name
//! them would only be able to misuse them.
//!
//! The reserved-word gate is the useful part. A field name comes from
//! contract.wit, so a collision produces a shader that will not parse and an
//! error pointing at generated source the author never wrote. Refusing early,
//! by name, is far kinder, and this list is the
//! one the renderer refuses by.

use crate::parse::Contract;
use crate::{snake, upper_camel};
use anyhow::{bail, Result};
use std::fmt::Write;

const RESERVED: &[&str] = &[
    // Keywords.
    "alias", "break", "case", "const", "const_assert", "continue", "continuing",
    "default", "diagnostic", "discard", "else", "enable", "false", "fn", "for",
    "if", "let", "loop", "override", "requires", "return", "struct", "switch",
    "true", "var", "while",
    // Reserved words. The whole list from the spec, not the handful that have
    // bitten so far: `active` was not in the short version, and it cost a
    // shader-compile error pointing at generated source instead of a message
    // naming the field and the file to rename it in.
    "NULL", "Self", "abstract", "active", "alignas", "alignof", "as", "asm",
    "asm_fragment", "async", "attribute", "auto", "await", "become",
    "binding_array", "cast", "catch", "class", "co_await", "co_return",
    "co_yield", "coherent", "column_major", "common", "compile",
    "compile_fragment", "concept", "const_cast", "consteval", "constexpr",
    "constinit", "crate", "debugger", "decltype", "delete", "demote",
    "demote_to_helper", "do", "dynamic_cast", "enum", "explicit", "export",
    "extends", "extern", "external", "fallthrough", "filter", "final",
    "finally", "friend", "from", "fxgroup", "get", "goto", "groupshared",
    "highp", "impl", "implements", "import", "inline", "instanceof",
    "interface", "layout", "lowp", "macro", "macro_rules", "match", "mediump",
    "meta", "mod", "module", "move", "mut", "mutable", "namespace", "new",
    "nil", "noexcept", "noinline", "nointerpolation", "non_coherent",
    "noncoherent", "noperspective", "null", "nullptr", "of", "operator",
    "package", "packoffset", "partition", "pass", "patch", "pixelfragment",
    "precise", "precision", "premerge", "priv", "protected", "pub", "public",
    "readonly", "ref", "regardless", "register", "reinterpret_cast", "require",
    "resource", "restrict", "self", "set", "shared", "sizeof", "smooth",
    "snorm", "static", "static_assert", "static_cast", "std", "subroutine",
    "super", "target", "template", "this", "thread_local", "throw", "trait",
    "try", "type", "typedef", "typeid", "typename", "typeof", "union",
    "unless", "unorm", "unsafe", "unsized", "use", "using", "varying",
    "virtual", "volatile", "wgsl", "where", "with", "writeonly", "yield",
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
        if r.fields.len() > 14 {
            bail!(
                "instance input `{}` has {} fields; locations 0..13 are the limit, 14 and 15 belong to MeshVertex",
                r.name,
                r.fields.len()
            );
        }
        let _ = writeln!(out, "struct {}Instance {{", upper_camel(&r.name));
        for (i, f) in r.fields.iter().enumerate() {
            let _ = writeln!(out, "    @location({i}) {}: {},", snake(&f.name), f.ty.wgsl());
        }
        out.push_str("};\n\n");
    }
    out.push_str(
        "// --- mesh vertices and the texture a draw binds ---\n\n\
         struct MeshVertex { @location(14) pos: vec3<f32>, @location(15) normal: vec3<f32> };\n\
         @group(0) @binding(1) var se_tex: texture_2d<f32>;\n\
         @group(0) @binding(2) var se_samp: sampler;\n",
    );
    Ok(out)
}
