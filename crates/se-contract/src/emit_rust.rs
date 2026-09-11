//! `contract.rs` — the contract as Rust every module and the host compiles.
//!
//! Two properties are worth more than the code that produces them.
//!
//! The structs contain no pointer and no `usize`, so a module built for
//! wasm32 and a host built for x86_64 derive byte-identical layouts from one
//! file. That is why `span` and `column` are pairs of `u32` rather than the
//! `Str`/`Slice` the old ABI passed: a pointer is 4 bytes on one side and 8 on
//! the other.
//!
//! And every offset is asserted at compile time. `#[repr(C)]` agreeing with
//! WGSL is not a claim in a comment — it is a build error when it stops being
//! true.

use crate::arena::{Map, BLOB_SLOTS, MAX_COMMANDS};
use crate::layout::Record;
use crate::parse::Contract;
use crate::{snake, upper_camel};
use std::fmt::Write;

const HEADER: &str = "\
// generated from contract.wit by `se-contract` — do not edit
//
// Offsets are WGSL std430: the GPU decides the layout and `#[repr(C)]` is
// padded into agreeing, which is what makes a component column uploadable as
// instance data with no repacking. The `const` assertions below fail the build
// if that ever stops holding.
";

fn emit_record(out: &mut String, r: &Record) {
    let ty = upper_camel(&r.name);
    let _ = writeln!(out, "/// From `interface {}` of contract.wit.", r.iface);
    // `align(N)` is not decoration: WGSL aligns `vec3` to 16 while `[f32; 3]`
    // aligns to 4, so without it the record's stride in an array disagrees
    // with the shader's.
    let _ = writeln!(out, "#[repr(C, align({}))]", r.align);
    out.push_str("#[allow(dead_code)]\n#[derive(Clone, Copy, Debug, PartialEq)]\n");
    let _ = writeln!(out, "pub struct {ty} {{");

    let gaps = r.gaps();
    let mut pad = 0;
    for (i, f) in r.fields.iter().enumerate() {
        if let Some((_, width)) = gaps.iter().find(|(at, _)| *at == i) {
            let _ = writeln!(out, "    _pad{pad}: [u8; {width}],");
            pad += 1;
        }
        let _ = writeln!(out, "    pub {}: {},", snake(&f.name), f.ty.rust());
    }
    if let Some((_, width)) = gaps.iter().find(|(at, _)| *at == r.fields.len()) {
        let _ = writeln!(out, "    _pad{pad}: [u8; {width}],");
    }
    out.push_str("}\n\n");

    let _ = writeln!(out, "#[allow(dead_code)]\nimpl {ty} {{");
    let _ = writeln!(out, "    pub const SIZE: u32 = {};", r.size);
    let _ = writeln!(out, "    pub const ALIGN: u32 = {};", r.align);
    out.push_str("    /// Zeroed, so a module with no allocator can still make one.\n");
    let _ = writeln!(out, "    pub const ZERO: {ty} = unsafe {{ core::mem::zeroed() }};");
    out.push_str("}\n\n");

    out.push_str("const _: () = {\n");
    let _ = writeln!(out, "    assert!(core::mem::size_of::<{ty}>() == {});", r.size);
    let _ = writeln!(out, "    assert!(core::mem::align_of::<{ty}>() == {});", r.align);
    for f in &r.fields {
        let _ = writeln!(
            out,
            "    assert!(core::mem::offset_of!({ty}, {}) == {});",
            snake(&f.name),
            f.offset
        );
    }
    out.push_str("};\n\n");
}

fn emit_arena(out: &mut String, c: &Contract, m: &Map) {
    out.push_str(
        "/// Where everything sits in the shared linear memory.\n\
         ///\n\
         /// `BASE` must equal the `-z stack-size` every module is linked with: a\n\
         /// module's stack is at `[0, stack-size)` and `--global-base` does not move\n\
         /// it, so anything placed below `BASE` is inside every module's stack.\n\
         #[allow(dead_code)]\npub mod arena {\n",
    );
    let _ = writeln!(out, "    pub const BASE: u32 = 0x{:08x};", m.base);
    let _ = writeln!(out, "    pub const STACK_SIZE: u32 = BASE;");
    for r in &m.control {
        let _ = writeln!(out, "    /// `interface control` — {} bytes.", r.len);
        let _ = writeln!(out, "    pub const {}: u32 = 0x{:08x};", r.name.to_uppercase(), r.at);
    }
    out.push_str("    /// `render::frame`, rewritten by render.wasm each frame.\n");
    let _ = writeln!(out, "    pub const FRAME: u32 = 0x{:08x};", m.frame);
    out.push_str("    /// The command buffer `frame.commands` points into.\n");
    let _ = writeln!(out, "    pub const COMMANDS: u32 = 0x{:08x};", m.commands);
    let _ = writeln!(out, "    pub const MAX_COMMANDS: u32 = {MAX_COMMANDS};");
    out.push_str("    /// One `pool` descriptor per pool: the length its columns share.\n");
    let _ = writeln!(out, "    pub const POOLS: u32 = 0x{:08x};", m.pools);
    out.push_str("    /// One `column` descriptor per component.\n");
    let _ = writeln!(out, "    pub const COLUMNS: u32 = 0x{:08x};", m.columns);
    out.push_str("    /// One `span` per blob slot.\n");
    let _ = writeln!(out, "    pub const BLOBS: u32 = 0x{:08x};", m.blobs);
    let _ = writeln!(out, "    pub const BLOB_SLOTS: u32 = {BLOB_SLOTS};");
    out.push_str("    /// Host-managed from here. No address past this is baked.\n");
    let _ = writeln!(out, "    pub const HEAP: u32 = 0x{:08x};", m.heap);

    out.push_str("\n    /// Index into the column table.\n");
    for (i, r) in c.components().iter().enumerate() {
        let _ = writeln!(out, "    pub const COL_{}: u32 = {i};", r.name.to_uppercase());
    }
    let _ = writeln!(out, "    pub const N_COLUMNS: u32 = {};", c.components().len());

    out.push_str("\n    /// Index into the pool table.\n");
    for (i, p) in c.pools.iter().enumerate() {
        let _ = writeln!(out, "    pub const POOL_{}: u32 = {i};", p.name.to_uppercase());
    }
    let _ = writeln!(out, "    pub const N_POOLS: u32 = {};", c.pools.len());
    out.push_str("}\n\n");
}

/// The columns of each pool, so the host can place them and a module can walk
/// them without naming each one.
fn emit_pools(out: &mut String, c: &Contract) {
    let cols = c.components();
    for p in &c.pools {
        let idx: Vec<String> = p
            .columns
            .iter()
            .map(|name| {
                let i = cols.iter().position(|r| &r.name == name).expect("checked");
                format!("arena::COL_{}", cols[i].name.to_uppercase())
            })
            .collect();
        let _ = writeln!(
            out,
            "/// Pool `{}`: these columns share one length and are row-aligned.\n\
             /// Row `i` means the same thing in every one of them.",
            p.name
        );
        let _ = writeln!(out, "#[allow(dead_code)]\npub mod pool_{} {{", snake(&p.name));
        let _ = writeln!(out, "    use super::arena;");
        let _ = writeln!(out, "    pub const INDEX: u32 = arena::POOL_{};", p.name.to_uppercase());
        let _ = writeln!(out, "    pub const COLUMNS: [u32; {}] = [{}];", idx.len(), idx.join(", "));
        out.push_str("}\n\n");
    }
}

/// The vertex layout, from the same record. A column is uploaded as instance
/// data verbatim, which is only sound if these offsets *are* the record's.
fn emit_vertex(out: &mut String, c: &Contract) {
    for r in c.components() {
        let _ = writeln!(out, "/// Instance layout for a `{}` column.", r.name);
        let _ = writeln!(out, "#[allow(dead_code)]\npub mod vertex_{} {{", snake(&r.name));
        let _ = writeln!(out, "    pub const STRIDE: u64 = {};", r.size);
        out.push_str("    /// `(shader_location, byte_offset, lanes)`\n");
        let _ = writeln!(out, "    pub const ATTRS: [(u32, u64, u32); {}] = [", r.fields.len());
        for (i, f) in r.fields.iter().enumerate() {
            let _ = writeln!(out, "        ({i}, {}, {}),", f.offset, f.ty.lanes());
        }
        out.push_str("    ];\n");
        let _ = writeln!(out, "    pub const WGSL_STRUCT: &str = \"{}Instance\";", upper_camel(&r.name));
        out.push_str("}\n\n");
    }
}

pub fn emit(c: &Contract, m: &Map) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for r in &c.records {
        emit_record(&mut out, r);
    }
    emit_arena(&mut out, c, m);
    emit_pools(&mut out, c);
    emit_vertex(&mut out, c);
    out
}
