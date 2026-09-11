//! The host does not compile a game's contract in — it reads it. So the table
//! that crosses that gap has to survive the trip exactly, and this is where
//! that is checked.

use se_contract::layout::Ty;
use se_contract::read::Table;
use se_contract::table;
use se_contract::wasm;
use std::path::PathBuf;

fn gen() -> se_contract::Generated {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/game1.wit");
    se_contract::generate(&wit).expect("game1.wit generates")
}

#[test]
fn contract_wasm_is_a_wasm_module_with_one_section() {
    let g = gen();
    assert_eq!(&g.module[..4], b"\0asm", "must open like a wasm module");
    let names = wasm::section_names(&g.module).expect("sections");
    assert_eq!(names.len(), 1, "a contract carries exactly one section");
    assert_eq!(names[0].0, table::SECTION);
}

#[test]
fn every_record_survives_the_trip() {
    let g = gen();
    let bytes = wasm::section(&g.module, table::SECTION).expect("section");
    let t = Table::new(bytes).expect("table");

    let back = t.records().expect("records");
    assert_eq!(back.len(), g.contract.records.len());

    // Compare against the source of truth, record by record and field by
    // field. A table that lost an offset would still be a valid table.
    for r in &g.contract.records {
        let b = back.iter().find(|b| b.name == r.name).expect(&r.name);
        assert_eq!(b.size, r.size, "{} size", r.name);
        assert_eq!(b.align, r.align, "{} align", r.name);
        assert_eq!(b.fields.len(), r.fields.len(), "{} field count", r.name);
        for (i, f) in r.fields.iter().enumerate() {
            assert_eq!(b.fields[i].name, f.name, "{}.{}", r.name, f.name);
            assert_eq!(b.fields[i].ty, f.ty, "{}.{} type", r.name, f.name);
            assert_eq!(b.fields[i].offset, f.offset, "{}.{} offset", r.name, f.name);
        }
    }
}

#[test]
fn a_column_index_is_a_record_index() {
    let g = gen();
    let bytes = wasm::section(&g.module, table::SECTION).unwrap();
    let t = Table::new(bytes).unwrap();

    // Components are emitted first so that the index the pool table carries,
    // the index `arena::COL_*` names, and the index into `components()` are
    // all one number.
    let comps = t.components().expect("components");
    let names: Vec<&str> = comps.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["breathe", "orbit", "spin", "transform"]);

    let pools = t.pools().expect("pools");
    assert_eq!(pools.len(), 1);
    assert_eq!(pools[0].name, "bodies");
    let mut cols = pools[0].columns.clone();
    cols.sort();
    assert_eq!(cols, [0, 1, 2, 3], "the pool names all four columns");

    let transform = &comps[3];
    assert_eq!(transform.name, "transform");
    assert_eq!(transform.fields[1].offset, 16, "rot, read back from bytes");
    assert_eq!(transform.fields[1].ty, Ty::Vec(4));
}

#[test]
fn the_arena_map_crosses_too() {
    let g = gen();
    let bytes = wasm::section(&g.module, table::SECTION).unwrap();
    let t = Table::new(bytes).unwrap();
    assert_eq!(t.arena_base(), g.map.base);
    assert_eq!(t.arena_columns(), g.map.columns);
    assert_eq!(t.arena_pools(), g.map.pools);
    assert_eq!(t.arena_heap(), g.map.heap);
    assert_eq!(t.arena_frame(), g.map.frame);

    // And the control var set is named, so a host that compiled no contract
    // can still say where `look` is.
    let ctl = t.control().expect("control");
    let look = ctl.iter().find(|c| c.name == "look").expect("look");
    assert_eq!(look.len, 12);
    assert!(look.at >= t.arena_base());
}

#[test]
fn a_truncated_table_is_refused_rather_than_misread() {
    let g = gen();
    let bytes = wasm::section(&g.module, table::SECTION).unwrap();
    assert!(Table::new(&bytes[..8]).is_err(), "too short for a header");
    let mut bad = bytes.to_vec();
    bad[0] = b'X';
    assert!(Table::new(&bad).is_err(), "wrong magic");
}
