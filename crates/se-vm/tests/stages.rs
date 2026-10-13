//! `stages::resolve`: the six ways a stage is refused, and the way it is not.

mod common;

use se_vm::stages::{self, Stage, READ, WRITE};
use se_vm::world::Meta;

fn stage(name: &str, params: &[(&str, u32)]) -> Stage {
    let params: Vec<(&str, u32, u32)> = params.iter().map(|&(n, a)| (n, a, 0)).collect();
    let mut v = stages::decode(&stages::encode(name, stages::ROW, &params)).expect("decode");
    assert_eq!(v.len(), 1);
    v.remove(0)
}

fn refused(meta: &Meta, name: &str, params: &[(&str, u32)]) -> String {
    let mut s = [stage(name, params)];
    stages::resolve(&mut s, meta).unwrap_err().to_string()
}

#[test]
fn happy_path_sets_columns() {
    let meta = common::meta();
    let mut s = [stage("orbit", &[("orbit", READ), ("transform", WRITE)])];
    stages::resolve(&mut s, &meta).expect("resolves");
    assert_eq!(s[0].params[0].column, meta.column_index("orbit").unwrap());
    assert_eq!(s[0].params[1].column, meta.column_index("transform").unwrap());
}

#[test]
fn empty_param_list_is_refused() {
    let msg = refused(&common::meta(), "nothing", &[]);
    assert!(msg.contains("names no column"), "{msg}");
    assert!(msg.contains("nothing"), "{msg}");
}

#[test]
fn unknown_component_is_refused() {
    let msg = refused(&common::meta(), "haunt", &[("ghost", READ)]);
    assert!(msg.contains("does not declare"), "{msg}");
    assert!(msg.contains("haunt") && msg.contains("ghost"), "{msg}");
}

#[test]
fn duplicate_component_is_refused() {
    let msg = refused(&common::meta(), "dup", &[("orbit", READ), ("orbit", WRITE)]);
    assert!(msg.contains("twice"), "{msg}");
    assert!(msg.contains("dup") && msg.contains("orbit"), "{msg}");
}

#[test]
fn bad_access_is_refused() {
    let msg = refused(&common::meta(), "odd", &[("orbit", 7)]);
    assert!(msg.contains("gives"), "{msg}");
    assert!(msg.contains("odd") && msg.contains("orbit") && msg.contains('7'), "{msg}");
}

#[test]
fn stage_across_two_pools_is_refused() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit"),
    )
    .unwrap();
    let src = src
        .replace(
            "record breathe ",
            "record glow { v: f32 }\n  record breathe ",
        )
        .replace(
            "record bodies {",
            "record lamps { g: glow }\n  record bodies {",
        )
        .replace("use components.{transform,", "use components.{glow, transform,");
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("stages_two_pools");
    std::fs::create_dir_all(&dir).unwrap();
    let wit = dir.join("two.wit");
    std::fs::write(&wit, src).unwrap();
    let g = se_contract::generate(&wit).expect("two pools generate");
    let bytes = se_contract::wasm::section(&g.module, se_contract::table::SECTION).unwrap();
    let meta = Meta::read(&se_contract::read::Table::new(bytes).unwrap()).unwrap();
    assert!(meta.pools.len() >= 2);

    let msg = refused(&meta, "cross", &[("orbit", READ), ("glow", READ)]);
    assert!(msg.contains("more than one pool"), "{msg}");
    assert!(msg.contains("cross"), "{msg}");
}
