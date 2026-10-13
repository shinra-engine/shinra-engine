//! A record may carry a string, a list or a span; each lowers to one 8-byte
//! `Span` and the record size never depends on the payload.

use se_contract::layout::Ty;
use se_contract::read::Table;
use se_contract::{table, wasm};
use std::path::PathBuf;

fn wit(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join(name)
}

/// spans.wit with `extra` spliced in as one more control record.
fn with_extra(extra: &str) -> PathBuf {
    let mut s = std::fs::read_to_string(wit("spans.wit")).unwrap();
    s = s.replace("  record intent", &format!("  {extra}\n  record intent"));
    s = s.replace(
        "  use shinra:engine/types.{vec3};\n",
        "  use shinra:engine/types.{vec3};\n  use shinra:engine/handles.{span};\n",
    );
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let p = dir.join(format!("spans_{:x}.wit", extra.len()));
    std::fs::write(&p, s).unwrap();
    p
}

#[test]
fn layout() {
    let g = se_contract::generate(&wit("spans.wit")).expect("spans.wit generates");
    let note = g.contract.records.iter().find(|r| r.name == "note").unwrap();
    let at = |n: &str| note.fields.iter().find(|f| f.name == n).unwrap();
    assert_eq!(at("text").offset, 0);
    assert_eq!(at("cues").offset, 8);
    assert_eq!(at("tag").offset, 16);
    assert_eq!(at("text").ty, Ty::Span);
    assert_eq!(at("cues").ty, Ty::Span);
    assert_eq!((note.size, note.align), (20, 4));
}

#[test]
fn refuses() {
    let e = se_contract::generate(&with_extra("record bad { x: list<list<u32>> }"))
        .err().expect("refused");
    assert!(format!("{e:#}").contains("list"), "{e:#}");
    let e = se_contract::generate(&with_extra("record bad { x: list<string> }"))
        .err().expect("refused");
    assert!(format!("{e:#}").contains("list"), "{e:#}");
}

#[test]
fn span_field_and_table() {
    let g = se_contract::generate(&with_extra("record held { s: span }"))
        .expect("an explicit span field is accepted");
    let held = g.contract.records.iter().find(|r| r.name == "held").unwrap();
    assert_eq!(held.fields[0].ty, Ty::Span);
}

#[test]
fn table() {
    let g = se_contract::generate(&wit("spans.wit")).unwrap();
    let bytes = wasm::section(&g.module, table::SECTION).expect("section");
    let back = Table::new(bytes).unwrap().records().unwrap();
    let note = back.iter().find(|r| r.name == "note").unwrap();
    assert_eq!(note.fields[0].ty, Ty::Span);
    assert_eq!(note.fields[1].ty, Ty::Span);
    assert_eq!(note.fields[2].ty, Ty::U32);
    assert_eq!(table::ty_code(Ty::Span), 3);
}
