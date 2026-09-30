//! A string and a list cross from the host into a real wasm module through
//! spans, and a snapshot keeps them. Built the way `frame.rs` builds modules.

use anyhow::{bail, Result};
use se_vm::memory::{Slot, STACK_SIZE};
use se_vm::Vm;
use std::path::{Path, PathBuf};
use std::process::Command;

fn has_wasm_target() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
}

fn build(dir: &Path, name: &str, slot: Slot, src: &str) -> Result<Vec<u8>> {
    let file = dir.join(format!("{name}.rs"));
    std::fs::write(&file, src)?;
    let out = dir.join(format!("{name}.wasm"));
    let o = Command::new("rustc")
        .args(["--edition", "2021", "--target", "wasm32-unknown-unknown"])
        .args(["--crate-type", "cdylib", "--crate-name", name])
        .arg("-C").arg("link-arg=--import-memory")
        .arg("-C").arg(format!("link-arg=--global-base={}", slot.global_base()))
        .arg("-C").arg(format!("link-arg=-zstack-size={STACK_SIZE}"))
        .arg("-C").arg("opt-level=1")
        .arg("-C").arg("panic=abort")
        .arg("-o").arg(&out)
        .arg(&file)
        .output()?;
    if !o.status.success() {
        bail!("building {name}:\n{}", String::from_utf8_lossy(&o.stderr));
    }
    Ok(std::fs::read(out)?)
}

fn scratch(what: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "se-vm-{what}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

const GAME: &str = r#"
#![no_std]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }
include!("contract.rs");

/// Reads blob slot 0 as text and slot 1 as a list, and reports both through
/// the `note` control var. No import: the span is read in place.
#[no_mangle]
pub extern "C" fn tick(_n: u32) -> u32 {
    unsafe {
        let text = *((arena::BLOBS + 0 * 8) as *const Span);
        let cues = *((arena::BLOBS + 1 * 8) as *const Span);
        let note = &mut *(arena::NOTE as *mut Note);
        note.tag = text.as_str().map_or(u32::MAX, |s| s.len() as u32);
        note.cues = cues;
        0
    }
}
"#;

#[test]
fn a_module_reads_a_string_and_a_list_and_a_snapshot_keeps_them() -> Result<()> {
    if !has_wasm_target() {
        eprintln!("skipping: rustup target add wasm32-unknown-unknown");
        return Ok(());
    }
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/spans.wit");
    let g = se_contract::generate(&wit)?;
    let dir = scratch("spans");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("contract.rs"), &g.rust)?;
    let game = build(&dir, "game", Slot::Game, GAME)?;

    let mut vm = Vm::new(&g.module, 16)?;
    vm.load(Slot::Game, "game", game)?;
    vm.put_str(0, "hello span")?;
    vm.put_u32s(1, &[3, 5, 8])?;
    vm.frame(0)?;

    let (at, _) = vm.meta.control_at("note").expect("note is a control var");
    let note = vm.read(at, 20)?;
    let w = |i: usize| u32::from_le_bytes(note[i * 4..i * 4 + 4].try_into().unwrap());
    // note: text span @0, cues span @8, tag @16
    assert_eq!(w(4), 10, "the module saw 10 bytes of text");
    let list = vm.read(w(2), w(3))?;
    let sum: u32 = list.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).sum();
    assert_eq!(sum, 16, "the module passed on the list it read");

    let s = vm.snapshot();
    vm.put_str(0, "x")?;
    assert_eq!(vm.blob(0)?, b"x");
    vm.restore(&s)?;
    assert_eq!(vm.blob(0)?, b"hello span");
    assert_eq!(vm.blob(1)?.len(), 12);
    Ok(())
}
