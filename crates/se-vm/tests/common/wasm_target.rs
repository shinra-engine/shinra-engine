//! The wasm32 target is a precondition of the module-building tests, and a
//! missing precondition fails the test rather than passing it with nothing run.
//!
//! No crate imports: se-gpu and se-contract tests include this file by path.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

pub const TARGET: &str = "wasm32-unknown-unknown";

/// Why `rustc` cannot build for wasm32, or `None` if it can. Asks `rustc`
/// itself, not `rustup`: the build uses `rustc`.
pub fn wasm_target_problem(rustc: &str, sysroot: Option<&Path>) -> Option<String> {
    let fix = format!("run: rustup target add {TARGET}");
    let mut cmd = Command::new(rustc);
    cmd.args(["--print", "target-libdir", "--target", TARGET]);
    if let Some(dir) = sysroot {
        cmd.arg("--sysroot").arg(dir);
    }
    let out = match cmd.output() {
        Ok(out) => out,
        Err(e) => return Some(format!("cannot run {rustc}: {e}; {fix}")),
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Some(format!("{rustc} has no {TARGET}: {}; {fix}", err.trim()));
    }
    let lib = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let has_core = std::fs::read_dir(&lib).ok().is_some_and(|entries| {
        entries
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("libcore-"))
    });
    if has_core {
        None
    } else {
        Some(format!("{TARGET} is not installed for {rustc} (no libcore in {lib}); {fix}"))
    }
}

/// Panics unless the `rustc` the tests build with has the wasm32 std installed.
pub fn require_wasm_target() {
    if let Some(problem) = wasm_target_problem("rustc", None) {
        panic!("{problem}");
    }
}
