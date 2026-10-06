//! The generated `Span` helpers: present as text, and they compile for wasm32.

use std::path::PathBuf;
use std::process::Command;

#[path = "../../se-vm/tests/common/wasm_target.rs"]
mod wasm_target;

fn generated() -> String {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/spans.wit");
    se_contract::generate(&wit).expect("spans.wit generates").rust
}

#[test]
fn text() {
    let rs = generated();
    assert!(rs.contains("impl Span"), "{rs}");
    assert!(rs.contains("fn as_str"));
    assert!(rs.contains("fn list<T: Copy>"));
    assert!(rs.contains("fn bytes"));
    assert!(rs.contains("memory_size(0)"));
    assert!(rs.contains("#[cfg(target_arch = \"wasm32\")]"));
}

#[test]
fn wasm() {
    wasm_target::require_wasm_target();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let file = dir.join("span_rs_probe.rs");
    let src = format!(
        "#![no_std]\n{}\n#[allow(dead_code)]\npub unsafe fn probe(n: &Note) -> (Option<&'static str>, &'static [u32]) {{\n    (n.text.as_str(), n.cues.list::<u32>())\n}}\n",
        generated()
    );
    std::fs::write(&file, src).unwrap();
    let o = Command::new("rustc")
        .args(["--edition", "2021", "--target", "wasm32-unknown-unknown"])
        .args(["--crate-type", "rlib", "--crate-name", "span_rs_probe"])
        .arg("--out-dir").arg(&dir)
        .arg(&file)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}
