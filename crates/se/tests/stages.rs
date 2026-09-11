//! Marking a stage is registering it. `#[se::stage]` files the function in the
//! `se_stages` link section, and `se_register_stages` — written once, in `se`
//! — hands that whole section to the host. There is no second list, so there
//! is nothing for a name to fall out of step with.

/// Two stages and no `se::stages!` line anywhere. Both have to arrive.
mod marked {
    #[se::stage]
    fn drift(i: &se::Input, dt: f32) {
        let _ = (i.mouse_down, dt);
    }

    #[se::stage]
    fn settle(i: &se::Input) {
        let _ = i.mouse_down;
    }
}

/// The older spelling. It still stamps the ABI and still refuses a name that
/// is not a stage; what it no longer does is decide which stages exist.
mod named {
    #[se::stage]
    fn wake(i: &se::Input) {
        let _ = i.mouse_down;
    }

    se::stages!(wake);
}

use se::{Access, ParamKind, StageSpec};

/// What the host would get out of this image, in the order it would get it —
/// sorted, because the section's order is the linker's business and no stage
/// may depend on it.
fn registered() -> Vec<&'static StageSpec> {
    extern "C" {
        fn se_register_stages() -> *const se::Registered;
    }

    // Read back where the module put them, rather than being called per item.
    let mut got: Vec<&'static StageSpec> = unsafe {
        let r = &*se_register_stages();
        assert_eq!(r.stride as usize, core::mem::size_of::<StageSpec>());
        core::slice::from_raw_parts(r.ptr as *const StageSpec, r.len as usize)
            .iter()
            .collect()
    };
    got.sort_by_key(|s| unsafe { s.name.as_str() });
    got
}

fn name(s: &StageSpec) -> &'static str {
    unsafe { s.name.as_str() }
}

/// The point of the ticket: neither `marked` stage is named a second time, and
/// both are there anyway — beside the one that still is.
#[test]
fn marking_registers_every_stage() {
    let got = registered();
    let names: Vec<&str> = got.iter().map(|s| name(s)).collect();
    assert_eq!(names, ["drift", "settle", "wake"]);
}

/// A collected stage is the same spec `se::stages!` used to push by hand: the
/// signature, read off the parameter list, and a shim to run it with.
#[test]
fn a_stage_arrives_as_its_signature() {
    let got = registered();
    let drift = got.iter().find(|s| name(s) == "drift").expect("drift registered");

    let params = unsafe { drift.params.as_slice() };
    assert_eq!(params.len(), 2);
    assert_eq!(unsafe { params[0].name.as_str() }, se::Input::NAME);
    assert_eq!((params[0].kind, params[0].access), (ParamKind::Resource, Access::Read));
    assert_eq!(params[1].kind, ParamKind::Dt);

    // `settle` takes `&Input` and no dt — the query is the signature, not a
    // shape every stage shares.
    let settle = got.iter().find(|s| name(s) == "settle").expect("settle registered");
    assert_eq!(unsafe { settle.params.as_slice() }.len(), 1);
}
