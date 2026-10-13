//! `contract.wit` → `contract.rs` + `contract.wgsl`.
//!
//! design.md: *所有的game都需要引用contract.rs並且是#[repr(C)] 並且我們以遵守
//! WGSL 的 std140/std430 為主*. One declaration, three consumers — every
//! module, the host, and every shader — and no way for them to disagree,
//! because none of them writes the layout down.
//!
//! WIT is the authority on *what*; it has no fixed-size arrays and no layout
//! control, and that is the right division. WGSL is the authority on *where*.
//! This crate is only the place the two meet.
//!
//! It deliberately does not use the component model. Components cannot share a
//! linear memory and their canonical ABI copies every value across a boundary,
//! which is the thing this engine exists to avoid — see `docs/`.

pub mod arena;
pub mod emit_rust;
pub mod emit_wgsl;
pub mod layout;
pub mod parse;
pub mod read;
pub mod stages;
pub mod table;
pub mod wasm;

use anyhow::Result;
use std::path::Path;

/// WIT spells names in kebab-case. Neither Rust nor WGSL takes a `-`, so the
/// conversion happens once, here, and every emitter uses the same answer.
pub fn snake(s: &str) -> String {
    s.replace('-', "_")
}

pub fn upper_camel(s: &str) -> String {
    s.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

pub struct Generated {
    pub rust: String,
    pub wgsl: String,
    /// `contract.wasm`: the same contract as a table the host reads at run
    /// time, so one engine binary can run any game.
    pub module: Vec<u8>,
    pub contract: parse::Contract,
    pub map: arena::Map,
    /// Fingerprint of the contract table; every module built from it carries it.
    pub id: u64,
}

pub fn generate(wit: &Path) -> Result<Generated> {
    let contract = parse::parse(wit)?;
    let map = arena::map(&contract);
    let table_bytes = table::encode(&contract, &map);
    let id = table::fingerprint(&table_bytes);
    let rust = emit_rust::emit(&contract, &map, id);
    let wgsl = emit_wgsl::emit(&contract)?;
    let module = wasm::wrap(table::SECTION, &table_bytes);
    Ok(Generated { rust, wgsl, module, contract, map, id })
}
