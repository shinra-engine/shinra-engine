//! `se::components!` must be a *spelling* of the hand-written form, not a
//! second dialect. The proof is `Layout::HASH`: it folds the name, size,
//! align and every field's name/type/offset/count, so two layouts agreeing on
//! it are interchangeable by the host's own definition.

/// The generated spelling. One act — the struct and its registration.
mod generated {
    se::components! {
        Transform {
            pos: [f32; 3],
            rot: [f32; 4],
            scale: [f32; 3],
        }

        Body {
            vel: [f32; 3],
            mass: f32,
        }
    }
}

/// The hand-written spelling this replaces, minus the `se::layouts!(..)` line
/// — a second `se_register_layouts` would collide at link time, which is
/// itself the duplication the macro exists to remove.
mod handwritten {
    #[repr(C)]
    #[derive(Clone, Copy, Debug, se::Schema)]
    pub struct Transform {
        pub pos: [f32; 3],
        pub rot: [f32; 4],
        pub scale: [f32; 3],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, se::Schema)]
    pub struct Body {
        pub vel: [f32; 3],
        pub mass: f32,
    }
}

use se::Schema;

fn assert_same<A: Schema, B: Schema>() {
    assert_eq!(A::NAME, B::NAME);
    assert_eq!(A::HASH, B::HASH, "{} hashes differ", A::NAME);

    let (a, b) = (A::layout(), B::layout());
    assert_eq!((a.size, a.align), (b.size, b.align));
    assert_eq!(a.hash, b.hash);

    // Safety: both layouts point into this test binary, which is loaded.
    let (af, bf) = unsafe { (a.fields.as_slice(), b.fields.as_slice()) };
    assert_eq!(af.len(), bf.len());
    for (x, y) in af.iter().zip(bf) {
        assert_eq!(unsafe { x.name.as_str() }, unsafe { y.name.as_str() });
        assert_eq!((x.ty, x.offset, x.count), (y.ty, y.offset, y.count));
    }
}

#[test]
fn generated_layout_matches_handwritten() {
    assert_same::<generated::Transform, handwritten::Transform>();
    assert_same::<generated::Body, handwritten::Body>();
}

/// Declaring is registering: the entry point exists and offers up *both*
/// components, without either being named a second time.
#[test]
fn declaring_registers_every_component() {
    extern "C" {
        fn se_register_layouts() -> *const se::Registered;
    }

    // The module hands back where its layouts live; the host reads them there.
    let got: &[se::Layout] = unsafe {
        let r = &*se_register_layouts();
        assert_eq!(r.stride as usize, core::mem::size_of::<se::Layout>());
        core::slice::from_raw_parts(r.ptr as *const se::Layout, r.len as usize)
    };

    let names: Vec<&str> = got.iter().map(|l| unsafe { l.name.as_str() }).collect();
    assert_eq!(names, ["Transform", "Body"]);

    assert_eq!(got[0].hash, <generated::Transform as Schema>::HASH);
    assert_eq!(got[1].hash, <generated::Body as Schema>::HASH);
}
