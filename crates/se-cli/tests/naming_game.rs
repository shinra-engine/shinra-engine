//! A game whose component is two words, built through the real CLI.
//!
//! `on-ground` is the name that used to emit `COL_ON-GROUND` and a stage table
//! entry the VM rejected; both ends have to agree on it now.

use std::path::{Path, PathBuf};
use std::process::Command;

const CONTRACT: &str = r#"package shinra:naming@0.1.0;

interface components {
  record on-ground { flag: u32, height: f32 }
}

interface pools {
  use components.{on-ground};

  record bodies { g: on-ground }
}

"#;

const FALL: &str = r#"use crate::*;

#[se::stage]
fn fall(g: &mut OnGround) {
    g.height -= 1.0;
    g.flag = 1;
}
"#;

fn has_wasm_target() -> bool {
    let Ok(o) = Command::new("rustc")
        .args(["--print", "target-libdir", "--target", "wasm32-unknown-unknown"])
        .output()
    else {
        return false;
    };
    o.status.success() && Path::new(String::from_utf8_lossy(&o.stdout).trim()).is_dir()
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shinra-naming-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("game/process")).unwrap();
    std::fs::write(dir.join("game/contract.wit"), CONTRACT).unwrap();
    std::fs::write(dir.join("game/process/fall.rs"), FALL).unwrap();
    dir
}

#[test]
fn multi_word_component_builds_and_stages() {
    if !has_wasm_target() {
        eprintln!("skipped: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir = scratch();
    let out = dir.join("out");
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let run = Command::new(env!("CARGO_BIN_EXE_shinra"))
        .arg("build")
        .arg(dir.join("game"))
        .arg("-o")
        .arg(&out)
        .env("SHINRA_ENGINE", &engine)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "shinra build failed:\n{stderr}");

    let rust = std::fs::read_to_string(out.join("contract.rs")).unwrap();
    assert!(rust.contains("COL_ON_GROUND"), "no COL_ON_GROUND in contract.rs");
    assert!(!rust.contains("ON-GROUND"), "hyphenated identifier in contract.rs");

    let wasm = std::fs::read(out.join("process/process.wasm")).unwrap();
    let stages = se_contract::wasm::section(&wasm, "se.stages").expect("se.stages section");
    assert!(!stages.is_empty());
    let text = String::from_utf8_lossy(stages);
    assert!(text.contains("on-ground"), "stage table names the WIT component: {text:?}");

    // Every module artifact carries the contract id its crate was built with.
    let table = se_contract::wasm::section(&std::fs::read(out.join("contract.wasm")).unwrap(), se_contract::table::SECTION)
        .expect("contract table")
        .to_vec();
    let id = se_contract::table::fingerprint(&table).to_le_bytes();
    let got = se_contract::wasm::section(&wasm, se_contract::table::CONTRACT_ID_SECTION).expect("se.contract-id section");
    assert_eq!(got, id);

    let _ = std::fs::remove_dir_all(&dir);
}
