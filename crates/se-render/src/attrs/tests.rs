//! What a layout is allowed to turn into.
//!
//! These tests pin the two translations in `attrs.rs` that nothing else can
//! check: the byte offsets handed to `wgpu` and the WGSL text handed to naga.
//! Both are only wrong at run time — a bad offset renders a garbled frame, a
//! bad name fails to compile inside source the author never wrote — so the
//! arithmetic is asserted here in bytes rather than looked at in a window.

use super::*;
use se_host::registry::FieldDef;
use wgpu::VertexFormat as F;

fn field(name: &str, ty: ScalarTy, offset: u32, count: u32) -> FieldDef {
    FieldDef { name: name.to_string(), ty, offset, count }
}

fn layout(name: &str, size: u32, align: u32, fields: Vec<FieldDef>) -> LayoutDef {
    LayoutDef { name: name.to_string(), size, align, fields, hash: 0 }
}

/// game1's `Transform`, field for field.
fn transform() -> LayoutDef {
    layout("Transform", 40, 4, vec![
        field("pos", ScalarTy::F32, 0, 3),
        field("rot", ScalarTy::F32, 12, 4),
        field("scale", ScalarTy::F32, 28, 3),
    ])
}

/// game1's `Camera`: the exact shape `#[repr(C)]` and WGSL disagree about.
fn camera() -> LayoutDef {
    layout("Camera", 40, 4, vec![
        field("eye", ScalarTy::F32, 0, 3),
        field("focus", ScalarTy::F32, 12, 3),
        field("fov", ScalarTy::F32, 24, 1),
        field("dist", ScalarTy::F32, 28, 1),
        field("yaw", ScalarTy::F32, 32, 1),
        field("pitch", ScalarTy::F32, 36, 1),
    ])
}

/// `(format, offset, location)` each — the whole of a vertex buffer layout.
fn attrs(i: &Instancing) -> Vec<(F, u64, u32)> {
    i.attributes
        .iter()
        .map(|a| (a.format, a.offset, a.shader_location))
        .collect()
}

/// `unwrap_err` would demand `Debug` on the success type; this does not.
fn err<T>(r: Result<T>) -> String {
    r.err().expect("expected a rejection, got a layout").to_string()
}

fn f32s(vals: &[f32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn read_f32(bytes: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

// ---- instancing: offsets and locations ---------------------------------

#[test]
fn vec_fields_land_at_their_repr_c_offsets() {
    let i = instancing(&transform()).unwrap();
    assert_eq!(
        attrs(&i),
        vec![
            (F::Float32x3, 0, 3),
            (F::Float32x4, 12, 4),
            (F::Float32x3, 28, 5),
        ]
    );
}

#[test]
fn instance_locations_start_after_the_mesh_vertex() {
    assert_eq!(INSTANCE_BASE, 3);
    let i = instancing(&transform()).unwrap();
    assert_eq!(i.attributes[0].shader_location, INSTANCE_BASE);
}

#[test]
fn instance_wgsl_names_every_field_once() {
    let i = instancing(&transform()).unwrap();
    assert_eq!(
        i.wgsl,
        "struct SeInstance {\n\
         \x20   @location(3) pos : vec3<f32>,\n\
         \x20   @location(4) rot : vec4<f32>,\n\
         \x20   @location(5) scale : vec3<f32>,\n\
         };\n"
    );
}

#[test]
fn a_scalar_field_is_not_a_vector() {
    let l = layout("Breathe", 4, 4, vec![field("base", ScalarTy::F32, 0, 1)]);
    let i = instancing(&l).unwrap();
    assert_eq!(attrs(&i), vec![(F::Float32, 0, 3)]);
    assert!(i.wgsl.contains("@location(3) base : f32,"));
}

#[test]
fn each_scalar_type_gets_its_own_format() {
    let cases = [
        (ScalarTy::F32, 1, F::Float32, "f32"),
        (ScalarTy::F32, 2, F::Float32x2, "vec2<f32>"),
        (ScalarTy::F32, 3, F::Float32x3, "vec3<f32>"),
        (ScalarTy::F32, 4, F::Float32x4, "vec4<f32>"),
        (ScalarTy::I32, 1, F::Sint32, "i32"),
        (ScalarTy::I32, 2, F::Sint32x2, "vec2<i32>"),
        (ScalarTy::I32, 3, F::Sint32x3, "vec3<i32>"),
        (ScalarTy::I32, 4, F::Sint32x4, "vec4<i32>"),
        (ScalarTy::U32, 1, F::Uint32, "u32"),
        (ScalarTy::U32, 2, F::Uint32x2, "vec2<u32>"),
        (ScalarTy::U32, 3, F::Uint32x3, "vec3<u32>"),
        (ScalarTy::U32, 4, F::Uint32x4, "vec4<u32>"),
    ];
    for (ty, count, format, wgsl) in cases {
        let l = layout("C", 16, 4, vec![field("v", ty, 0, count)]);
        let i = instancing(&l).unwrap();
        assert_eq!(attrs(&i), vec![(format, 0, 3)], "{} x{count}", ty.name());
        assert!(i.wgsl.contains(&format!("@location(3) v : {wgsl},")), "{}", i.wgsl);
    }
}

#[test]
fn every_shader_scalar_is_four_bytes() {
    // Why a split can step by `n * size` and land right for every type: the
    // three that survive `wgsl_scalar` are all the same width.
    for t in [ScalarTy::F32, ScalarTy::I32, ScalarTy::U32] {
        assert_eq!(t.size(), 4, "{}", t.name());
    }
}

#[test]
fn a_field_wider_than_four_splits_across_locations() {
    // mat4x4, at a non-zero offset so a split that forgot `f.offset` shows.
    let l = layout("M", 72, 4, vec![
        field("tint", ScalarTy::F32, 0, 2),
        field("m", ScalarTy::F32, 8, 16),
    ]);
    let i = instancing(&l).unwrap();
    assert_eq!(
        attrs(&i),
        vec![
            (F::Float32x2, 0, 3),
            (F::Float32x4, 8, 4),
            (F::Float32x4, 24, 5),
            (F::Float32x4, 40, 6),
            (F::Float32x4, 56, 7),
        ]
    );
    assert_eq!(
        i.wgsl,
        "struct SeInstance {\n\
         \x20   @location(3) tint : vec2<f32>,\n\
         \x20   @location(4) m_0 : vec4<f32>,\n\
         \x20   @location(5) m_1 : vec4<f32>,\n\
         \x20   @location(6) m_2 : vec4<f32>,\n\
         \x20   @location(7) m_3 : vec4<f32>,\n\
         };\n"
    );
}

#[test]
fn a_split_remainder_keeps_its_real_width() {
    let l = layout("M", 24, 4, vec![field("m", ScalarTy::F32, 0, 6)]);
    let i = instancing(&l).unwrap();
    assert_eq!(attrs(&i), vec![(F::Float32x4, 0, 3), (F::Float32x2, 16, 4)]);
    assert!(i.wgsl.contains("@location(4) m_1 : vec2<f32>,"), "{}", i.wgsl);
}

#[test]
fn exactly_four_is_not_split_and_keeps_its_name() {
    let l = layout("Q", 16, 4, vec![field("rot", ScalarTy::F32, 0, 4)]);
    let i = instancing(&l).unwrap();
    assert_eq!(attrs(&i), vec![(F::Float32x4, 0, 3)]);
    assert!(i.wgsl.contains("rot : vec4<f32>"));
    assert!(!i.wgsl.contains("rot_0"), "{}", i.wgsl);
}

#[test]
fn split_element_size_follows_the_scalar_not_f32() {
    let l = layout("Big", 40, 4, vec![field("m", ScalarTy::I32, 0, 10)]);
    let i = instancing(&l).unwrap();
    assert_eq!(
        attrs(&i),
        vec![(F::Sint32x4, 0, 3), (F::Sint32x4, 16, 4), (F::Sint32x2, 32, 5)]
    );
}

// ---- instancing: stride -------------------------------------------------

#[test]
fn stride_is_the_size_rounded_up_to_alignment() {
    assert_eq!(instancing(&transform()).unwrap().stride, 40);

    let l = layout("Pad", 12, 16, vec![field("pos", ScalarTy::F32, 0, 3)]);
    assert_eq!(instancing(&l).unwrap().stride, 16);

    let l = layout("Exact", 32, 16, vec![field("m", ScalarTy::F32, 0, 8)]);
    assert_eq!(instancing(&l).unwrap().stride, 32);
}

#[test]
fn a_zero_alignment_does_not_divide_by_zero() {
    let l = layout("Odd", 12, 0, vec![field("pos", ScalarTy::F32, 0, 3)]);
    assert_eq!(instancing(&l).unwrap().stride, 12);
}

// ---- rejections ---------------------------------------------------------

#[test]
fn u8_is_not_a_shader_input() {
    let l = layout("C", 4, 1, vec![field("flags", ScalarTy::U8, 0, 4)]);
    let e = err(instancing(&l));
    assert!(e.contains("u8"), "{e}");
    let e = err(uniform(&l));
    assert!(e.contains("u8") && e.contains("f32/i32/u32"), "{e}");
}

#[test]
fn f64_is_not_a_shader_input() {
    let l = layout("C", 8, 8, vec![field("t", ScalarTy::F64, 0, 1)]);
    let e = err(instancing(&l));
    assert!(e.contains("f64"), "{e}");
    let e = err(uniform(&l));
    assert!(e.contains("f64") && e.contains("f32/i32/u32"), "{e}");
}

#[test]
fn a_reserved_word_is_refused_by_name() {
    let l = layout("Beam", 12, 4, vec![field("target", ScalarTy::F32, 0, 3)]);
    for e in [
        err(instancing(&l)),
        err(uniform(&l)),
    ] {
        assert!(e.contains("Beam.target"), "{e}");
        assert!(e.contains("reserved word"), "{e}");
        assert!(e.contains("bundle/data.rs"), "{e}");
    }
}

#[test]
fn every_reserved_word_is_refused() {
    for word in RESERVED {
        let l = layout("C", 4, 4, vec![field(word, ScalarTy::F32, 0, 1)]);
        assert!(instancing(&l).is_err(), "`{word}` was accepted");
        assert!(uniform(&l).is_err(), "`{word}` was accepted");
    }
}

#[test]
fn engine_name_prefixes_are_refused() {
    for name in ["se_time", "__pad"] {
        let l = layout("C", 4, 4, vec![field(name, ScalarTy::F32, 0, 1)]);
        let e = err(instancing(&l));
        assert!(e.contains("reserved prefix"), "{e}");
        assert!(e.contains(&format!("C.{name}")), "{e}");
        assert!(uniform(&l).is_err());
    }
}

#[test]
fn an_ordinary_name_is_not_refused() {
    // `pos` and `scale` are near-misses against the predeclared list.
    for name in ["pos", "scale", "rate", "phase", "asset"] {
        let l = layout("C", 4, 4, vec![field(name, ScalarTy::F32, 0, 1)]);
        assert!(instancing(&l).is_ok(), "`{name}` was refused");
    }
}

// ---- uniform: repacking into 16-byte slots ------------------------------

#[test]
fn every_uniform_field_gets_a_whole_vec4_slot() {
    let u = uniform(&camera()).unwrap();
    assert_eq!(u.size, 6 * 16);
    assert_eq!(
        u.wgsl,
        "struct SeUniform {\n\
         \x20   eye : vec4<f32>,\n\
         \x20   focus : vec4<f32>,\n\
         \x20   fov : vec4<f32>,\n\
         \x20   dist : vec4<f32>,\n\
         \x20   yaw : vec4<f32>,\n\
         \x20   pitch : vec4<f32>,\n\
         };\n"
    );
}

#[test]
fn a_uniform_with_no_fields_is_still_one_slot() {
    let u = uniform(&layout("Empty", 0, 1, vec![])).unwrap();
    assert_eq!(u.size, 16);
    assert_eq!(u.wgsl, "struct SeUniform {\n};\n");
    assert_eq!(u.pack(&[]), vec![0u8; 16]);
}

#[test]
fn pack_moves_each_field_to_the_start_of_its_slot() {
    // The whole point: `focus` sits at byte 12 in Rust and byte 16 in WGSL.
    let src = f32s(&[
        1.0, 2.0, 3.0, // eye
        4.0, 5.0, 6.0, // focus
        0.5,  // fov
        7.0,  // dist
        8.0,  // yaw
        9.0,  // pitch
    ]);
    assert_eq!(src.len(), 40);

    let out = uniform(&camera()).unwrap().pack(&src);
    assert_eq!(out.len(), 96);

    for (slot, want) in [
        (0, [1.0, 2.0, 3.0].as_slice()),
        (1, &[4.0, 5.0, 6.0]),
        (2, &[0.5]),
        (3, &[7.0]),
        (4, &[8.0]),
        (5, &[9.0]),
    ] {
        let base = slot * 16;
        for (k, w) in want.iter().enumerate() {
            assert_eq!(read_f32(&out, base + k * 4), *w, "slot {slot} lane {k}");
        }
        // The rest of the slot is padding and must stay zero, or a shader
        // reading `.w` off a vec3 sees whatever the neighbour left behind.
        assert!(
            out[base + want.len() * 4..base + 16].iter().all(|b| *b == 0),
            "slot {slot} padding"
        );
    }
}

#[test]
fn a_wide_uniform_field_splits_into_consecutive_slots() {
    let l = layout("M", 32, 4, vec![field("m", ScalarTy::F32, 0, 8)]);
    let u = uniform(&l).unwrap();
    assert_eq!(u.size, 32);
    assert_eq!(
        u.wgsl,
        "struct SeUniform {\n\
         \x20   m_0 : vec4<f32>,\n\
         \x20   m_1 : vec4<f32>,\n\
         };\n"
    );

    let src = f32s(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    let out = u.pack(&src);
    assert_eq!(out, src, "a field already 16-aligned should pack unchanged");
}

#[test]
fn a_wide_uniform_field_pads_its_last_slot() {
    let l = layout("M", 24, 4, vec![field("m", ScalarTy::F32, 0, 6)]);
    let u = uniform(&l).unwrap();
    assert_eq!(u.size, 32);
    assert!(u.wgsl.contains("m_1 : vec4<f32>,"), "{}", u.wgsl);

    let out = u.pack(&f32s(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
    assert_eq!(read_f32(&out, 16), 5.0);
    assert_eq!(read_f32(&out, 20), 6.0);
    assert!(out[24..32].iter().all(|b| *b == 0));
}

#[test]
fn pack_leaves_a_short_source_alone_rather_than_panicking() {
    // A save file written against an older layout is short. Reading past it
    // would be a crash in the render loop; the slot stays zero instead.
    let u = uniform(&camera()).unwrap();
    let out = u.pack(&f32s(&[1.0, 2.0, 3.0]));
    assert_eq!(out.len(), 96);
    assert_eq!(read_f32(&out, 0), 1.0);
    assert!(out[16..].iter().all(|b| *b == 0));
}

#[test]
fn uniform_slot_offsets_do_not_depend_on_rust_padding() {
    // Same fields, same order, but `#[repr(C)]` put a hole before `b`.
    let packed = layout("P", 8, 4, vec![
        field("a", ScalarTy::F32, 0, 1),
        field("b", ScalarTy::F32, 4, 1),
    ]);
    let holed = layout("H", 16, 8, vec![
        field("a", ScalarTy::F32, 0, 1),
        field("b", ScalarTy::F32, 8, 1),
    ]);
    assert_eq!(uniform(&packed).unwrap().size, uniform(&holed).unwrap().size);

    let a = uniform(&packed).unwrap().pack(&f32s(&[1.0, 2.0]));
    let b = uniform(&holed).unwrap().pack(&f32s(&[1.0, 0.0, 2.0]));
    assert_eq!(a, b);
}
