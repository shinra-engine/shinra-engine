//! The wasm32 precondition check fails with the fix command, never skips.

mod common;

use common::wasm_target::{wasm_target_problem, TARGET};
use common::require_wasm_target;

#[test]
fn an_empty_sysroot_names_the_fix() {
    let dir = std::env::temp_dir().join(format!("se-vm-empty-sysroot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let problem = wasm_target_problem("rustc", Some(&dir));
    std::fs::remove_dir_all(&dir).ok();
    let problem = problem.expect("an empty sysroot has no wasm32 std");
    assert!(
        problem.contains(&format!("rustup target add {TARGET}")),
        "{problem}"
    );
}

#[test]
fn a_missing_rustc_is_a_problem() {
    let problem = wasm_target_problem("rustc-that-does-not-exist-xyz", None);
    assert!(problem.expect("no such rustc").ends_with("rustup target add wasm32-unknown-unknown"));
}

#[test]
fn this_machine_has_the_target() {
    require_wasm_target();
}
