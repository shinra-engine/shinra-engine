//! Both routes must read the same contract out of the same source.
//!
//! Not a unit test — it drives the real build of both pipelines and compares
//! what each reads back, which is the only check that means anything here. The
//! wasm side runs under node, because the route's real home is a browser and
//! this is how it gets exercised without one.
//!
//! Skipped when the tools are missing; loud when they are present and the
//! answer differs.

use std::path::{Path, PathBuf};
use std::process::Command;

fn engine() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

fn have(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn every_module_reads_the_same_on_both_routes() {
    if !have("node", &["--version"]) || !have("wasm-bindgen", &["--version"]) {
        eprintln!("skipping: needs node and wasm-bindgen");
        return;
    }
    let root = engine();
    let script = root.join("web/routes_agree.sh");
    assert!(script.exists(), "{} is missing", script.display());

    let out = Command::new("bash")
        .arg(&script)
        .args(["game1", "game2", "game3"])
        .current_dir(&root)
        .output()
        .expect("running the comparison");
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "routes disagree:\n{text}\n{err}");
    assert!(
        text.contains("all routes agree"),
        "the comparison did not run to completion:\n{text}\n{err}"
    );
}
