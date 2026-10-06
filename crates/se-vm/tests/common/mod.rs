//! Shared fixtures for the se-vm integration tests.
#![allow(dead_code)]

use se_contract::read::Table;
use se_contract::{table, wasm, Generated};
use se_vm::world::Meta;
use std::path::PathBuf;

mod wasm_target;
#[allow(unused_imports)]
pub use wasm_target::require_wasm_target;

/// `game1.wit` run through the generator: one pool, `bodies`.
pub fn game1() -> Generated {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    se_contract::generate(&wit).expect("game1.wit generates")
}

/// The contract table the host reads at run time.
pub fn meta() -> Meta {
    let g = game1();
    let bytes = wasm::section(&g.module, table::SECTION).expect("section");
    Meta::read(&Table::new(bytes).expect("table")).expect("meta")
}
