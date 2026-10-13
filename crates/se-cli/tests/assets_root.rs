//! Assets are named by constant: ids come from the sorted module names, and
//! each asset module says its kind.

use std::path::{Path, PathBuf};
use std::process::Command;

const CONTRACT: &str = "package shinra:assets@0.1.0;\n";

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
    let dir = std::env::temp_dir().join(format!("shinra-assets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let asset = dir.join("game/asset");
    std::fs::create_dir_all(asset.join("zed")).unwrap();
    std::fs::create_dir_all(asset.join("alpha")).unwrap();
    std::fs::write(dir.join("game/contract.wit"), CONTRACT).unwrap();
    std::fs::write(asset.join("zed/a.OBJ"), "v 0 0 0\n").unwrap();
    std::fs::write(asset.join("alpha/b.png"), "png").unwrap();
    std::fs::write(asset.join("zed.rs"), "se::asset!(\"zed/a.OBJ\");\n").unwrap();
    std::fs::write(asset.join("alpha.rs"), "se::asset!(\"alpha/b.png\");\n").unwrap();
    dir
}

#[test]
fn ids_follow_sorted_names_and_kinds_are_extensions() {
    if !has_wasm_target() {
        eprintln!("skipped: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir = scratch();
    let out = dir.join("out");
    let engine = std::env::var_os("SHINRA_ENGINE")
        .map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."), PathBuf::from);
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

    let root = std::fs::read_to_string(out.join(".gen/asset_alpha_root.rs")).unwrap();
    assert!(root.contains("pub const ALPHA: u32 = 0;"), "{root}");
    assert!(root.contains("pub const ZED: u32 = 1;"), "{root}");

    for (name, kind) in [("alpha", "png"), ("zed", "obj")] {
        let wasm = std::fs::read(out.join(format!("asset/{name}.wasm"))).unwrap();
        let k = se_contract::wasm::section(&wasm, "se.asset.kind").expect("se.asset.kind");
        assert_eq!(k, kind.as_bytes());
    }
}
