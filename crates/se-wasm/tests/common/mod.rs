//! Building one source both ways, shared by the tests that compare backends.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One source, compiled both ways.
pub struct Built {
    pub so: PathBuf,
    pub wasm: PathBuf,
}

pub fn engine_root() -> PathBuf {
    // crates/se-wasm/ -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

pub fn have_wasm_target() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
}

/// Build `se` for wasm32 if it is not there yet.
///
/// The test does this itself rather than skipping when it is missing: once the
/// target is installed, "could not build" is a real problem, and a test that
/// prints "skipping" and passes is indistinguishable from one that works.
pub fn ensure_se_for_wasm() {
    let out = Command::new("cargo")
        .current_dir(engine_root())
        .args(["build", "-p", "se", "--target", "wasm32-unknown-unknown"])
        .output()
        .expect("running cargo");
    assert!(
        out.status.success(),
        "building `se` for wasm32 failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn build_both(root: &Path, crate_name: &str, out: &Path) -> Built {
    let engine = engine_root();
    let host_deps = engine.join("target/debug/deps");
    let wasm_deps = engine.join("target/wasm32-unknown-unknown/debug/deps");

    let find = |dir: &Path, prefix: &str| -> Option<PathBuf> {
        std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix) && n.ends_with(".rlib"))
        })
    };
    let se_host_rlib = find(&host_deps, "libse-")
        .expect("no host libse-*.rlib — build the engine first");
    let se_wasm_rlib =
        find(&wasm_deps, "libse-").expect("no wasm libse-*.rlib after building it");

    let so = out.join(format!("{crate_name}.so"));
    let wasm = out.join(format!("{crate_name}.wasm"));

    let run = |mut c: Command, what: &str| {
        let o = c.output().expect("running rustc");
        assert!(
            o.status.success(),
            "{what} build failed:\n{}",
            String::from_utf8_lossy(&o.stderr)
        );
    };

    let mut c = Command::new("rustc");
    c.args(["--edition", "2021", "--crate-type", "cdylib", "--crate-name", crate_name])
        .arg("-L").arg(format!("dependency={}", host_deps.display()))
        .arg("--extern").arg(format!("se={}", se_host_rlib.display()))
        .arg("-o").arg(&so).arg(root);
    run(c, "native");

    let mut c = Command::new("rustc");
    c.args(["--edition", "2021", "--target", "wasm32-unknown-unknown"])
        .args(["--crate-type", "cdylib", "--crate-name", crate_name])
        // proc macros are host artefacts even for a wasm build, so both
        // dependency paths are needed.
        .arg("-L").arg(format!("dependency={}", wasm_deps.display()))
        .arg("-L").arg(format!("dependency={}", host_deps.display()))
        .arg("--extern").arg(format!("se={}", se_wasm_rlib.display()))
        .arg("-o").arg(&wasm).arg(root);
    run(c, "wasm32");
    Built { so, wasm }
}

