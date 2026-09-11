//! The layout is the contract, so it is asserted here rather than eyeballed.
//!
//! Every number below is WGSL std430 computed by hand from the spec, not
//! copied from the generator's output — otherwise the test only proves the
//! generator agrees with itself.

use se_contract::layout::Ty;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/game1.wit")
}

fn gen() -> se_contract::Generated {
    se_contract::generate(&fixture()).expect("game1.wit generates")
}

#[test]
fn vec3_forces_the_gaps_that_repr_c_would_not_leave() {
    let g = gen();
    let t = g.contract.component("transform").expect("transform");
    // vec3 aligns to 16 and sizes to 12, so `rot` cannot follow `pos` at 12.
    assert_eq!(t.fields[0].offset, 0, "pos");
    assert_eq!(t.fields[1].offset, 16, "rot");
    assert_eq!(t.fields[2].offset, 32, "scale");
    assert_eq!(t.size, 48);
    assert_eq!(t.align, 16);
    // Plain `#[repr(C)]` over [f32;3], [f32;4], [f32;3] would have given
    // 0/12/28 and size 40. That 8-byte difference is the whole reason the
    // generator exists.
}

#[test]
fn a_scalar_fills_a_vec3s_tail_padding() {
    let g = gen();
    let o = g.contract.component("orbit").expect("orbit");
    assert_eq!(o.fields[0].offset, 0, "center: vec3");
    assert_eq!(o.fields[1].offset, 12, "radius: f32 sits in the vec3's tail");
    assert_eq!(o.fields[2].offset, 16, "speed");
    assert_eq!(o.fields[3].offset, 20, "phase");
    assert_eq!(o.fields[4].offset, 24, "tilt");
    assert_eq!(o.size, 32);
}

#[test]
fn handles_are_pairs_of_u32_so_both_targets_agree() {
    let g = gen();
    for (name, size) in [("span", 8), ("column", 16), ("pool", 8)] {
        let r = g.contract.of("handles").find(|r| r.name == name).expect(name);
        assert_eq!(r.size, size, "{name}");
        assert_eq!(r.align, 4, "{name}");
        assert!(
            r.fields.iter().all(|f| f.ty == Ty::U32),
            "{name} must be all u32: a pointer is 4 bytes on wasm32 and 8 on the host"
        );
    }
}

#[test]
fn the_pool_names_the_columns_that_share_a_length() {
    let g = gen();
    assert_eq!(g.contract.pools.len(), 1);
    let p = &g.contract.pools[0];
    assert_eq!(p.name, "bodies");
    let mut cols = p.columns.clone();
    cols.sort();
    assert_eq!(cols, ["breathe", "orbit", "spin", "transform"]);
}

#[test]
fn the_arena_begins_exactly_at_the_stack_top() {
    let g = gen();
    // Measured, not assumed: `--global-base` does not move a module's stack,
    // so [0, stack-size) belongs to every module and a world placed below it
    // is destroyed once a call nests deep enough.
    assert_eq!(g.map.base, se_contract::arena::STACK_SIZE);
    assert!(g.map.heap > g.map.blobs);
    assert!(g.map.blobs > g.map.columns);
    assert!(g.map.columns > g.map.pools);
    assert!(g.map.commands > g.map.frame);
}

#[test]
fn the_emitted_rust_asserts_its_own_offsets() {
    let g = gen();
    assert!(g.rust.contains("#[repr(C, align(16))]"));
    assert!(g.rust.contains("assert!(core::mem::offset_of!(Transform, rot) == 16);"));
    assert!(g.rust.contains("pub const STRIDE: u64 = 48;"), "vertex stride");
}

#[test]
fn the_shader_sees_components_and_control_and_nothing_else() {
    let g = gen();
    assert!(g.wgsl.contains("struct Transform {"));
    assert!(g.wgsl.contains("struct Camera {"));
    assert!(g.wgsl.contains("struct TransformInstance {"));
    // `handles` and `render` are host/module plumbing; a shader that could
    // name them could only misuse them.
    assert!(!g.wgsl.contains("struct Span {"));
    assert!(!g.wgsl.contains("struct Command {"));
}
