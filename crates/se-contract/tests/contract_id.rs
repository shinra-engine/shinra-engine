//! The contract id: stable for one contract, different for another, and
//! spelled out in the generated `contract.rs`.

use std::path::PathBuf;

fn wit(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join(name)
}

#[test]
fn same_contract_same_id() {
    let a = se_contract::generate(&wit("game1.wit")).unwrap();
    let b = se_contract::generate(&wit("game1.wit")).unwrap();
    assert_eq!(a.id, b.id);
}

#[test]
fn different_contract_different_id() {
    let a = se_contract::generate(&wit("game1.wit")).unwrap();
    let b = se_contract::generate(&wit("spans.wit")).unwrap();
    assert_ne!(a.id, b.id);
}

#[test]
fn rust_carries_the_id() {
    let g = se_contract::generate(&wit("game1.wit")).unwrap();
    let line = format!("pub const CONTRACT_ID: u64 = 0x{:016x};", g.id);
    assert!(g.rust.contains(&line), "missing {line}");
}
