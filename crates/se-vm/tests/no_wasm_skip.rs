//! A missing wasm32 target fails the wasm tests; it never skips them.

mod common;

use common::wasm_target::{wasm_target_problem, TARGET};
use std::path::{Path, PathBuf};

/// The crates whose tests build and run wasm modules.
const WASM_CRATES: [&str; 3] = ["se-vm", "se-gpu", "se-contract"];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
#[should_panic(expected = "rustup target add wasm32-unknown-unknown")]
fn require_wasm_target_panics_with_the_fix() {
    let dir = std::env::temp_dir().join(format!("se-vm-no-skip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let problem = wasm_target_problem("rustc", Some(&dir));
    std::fs::remove_dir_all(&dir).ok();
    if let Some(problem) = problem {
        panic!("{problem}");
    }
}

#[test]
fn no_test_skips_for_a_missing_target() {
    let banned = [concat!("has_wasm", "_target"), concat!("skipping", ":"), concat!("skipped", ":")];
    let me = Path::new(file!()).file_name().unwrap();
    let mut files = Vec::new();
    for krate in WASM_CRATES {
        rust_files(&crates_dir().join(krate).join("tests"), &mut files);
    }
    assert!(!files.is_empty(), "no test files found under {}", crates_dir().display());
    let offending: Vec<String> = files
        .iter()
        .filter(|p| p.file_name() != Some(me))
        .filter(|p| {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            banned.iter().any(|b| text.contains(b))
        })
        .map(|p| p.display().to_string())
        .collect();
    assert!(offending.is_empty(), "tests that skip instead of failing: {offending:?}");
}

#[test]
fn the_toolchain_installs_the_target_where_tests_run() {
    let root = crates_dir().join("..");
    let docker = std::fs::read_to_string(root.join("Dockerfile")).expect("Dockerfile");
    assert!(docker.contains(&format!("rustup target add {TARGET}")), "Dockerfile lacks {TARGET}");
    let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).expect("ci.yml");
    assert!(ci.contains(TARGET), "ci.yml lacks {TARGET}");
}
