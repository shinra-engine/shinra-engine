//! A module carries the fingerprint of the contract it was built against, and
//! the Vm refuses one that does not match — before it touches memory.

mod common;
use common::{build, prelude, require_wasm_target};

use anyhow::Result;
use se_vm::memory::Slot;
use se_vm::Vm;

const GAME: &str = "#[no_mangle] pub extern \"C\" fn tick(_n: u32) -> u32 { 0 }\n";
const SECTION_LINE: &str =
    "#[used]\n#[link_section = \"se.contract-id\"]\npub static SE_CONTRACT_ID: [u8; 8] = CONTRACT_ID.to_le_bytes();\n";

fn setup(what: &str) -> Result<(Vm, std::path::PathBuf)> {
    let g = common::game1();
    let dir = std::env::temp_dir().join(format!("se-vm-cid-{what}-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("contract.rs"), &g.rust)?;
    Ok((Vm::new(&g.module, 16)?, dir))
}

#[test]
fn the_matching_module_loads() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("ok")?;
    let bytes = build(&dir, "game", Slot::Game, &format!("{}{GAME}", prelude()))?;
    vm.load(Slot::Game, "game", bytes)?;
    vm.frame(0)?;
    Ok(())
}

#[test]
fn a_module_built_against_another_contract_is_refused() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("stale")?;
    let stale = prelude().replace(
        "CONTRACT_ID.to_le_bytes()",
        "[0u8; 8]",
    );
    let bytes = build(&dir, "game", Slot::Game, &format!("{stale}{GAME}"))?;
    let err = vm.load(Slot::Game, "game", bytes.clone()).unwrap_err().to_string();
    assert!(err.contains("built against contract"), "{err}");
    assert!(err.contains("shinra build"), "{err}");
    assert!(vm.check_contract("game", &bytes).is_err());
    Ok(())
}

#[test]
fn a_module_with_no_contract_id_is_refused() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("none")?;
    let bare = prelude().replace(SECTION_LINE, "");
    assert_ne!(bare, prelude(), "the prelude carried the section line");
    let bytes = build(&dir, "game", Slot::Game, &format!("{bare}{GAME}"))?;
    let err = vm.load(Slot::Game, "game", bytes).unwrap_err().to_string();
    assert!(err.contains("carries no contract id"), "{err}");
    Ok(())
}
