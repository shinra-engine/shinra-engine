//! The wasm32 target is a precondition of the module-building tests, and a
//! missing precondition fails the test rather than passing it with nothing run.

use std::path::PathBuf;
use std::process::Command;

const TARGET: &str = "wasm32-unknown-unknown";

/// Panics unless the `rustc` the tests build with has the wasm32 std installed.
/// Asks `rustc` itself, not `rustup`: the build uses `rustc`.
pub fn require_wasm_target() {
    let out = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .expect("rustc --print sysroot runs");
    let sysroot = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let lib = sysroot.join("lib/rustlib").join(TARGET).join("lib");
    assert!(
        lib.is_dir(),
        "{TARGET} is not installed for rustc ({} missing); run: rustup target add {TARGET}",
        lib.display()
    );
}
