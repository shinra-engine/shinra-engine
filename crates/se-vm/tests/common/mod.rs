//! Shared fixtures for the se-vm integration tests.
#![allow(dead_code)]

use anyhow::{bail, Result};
use se_contract::read::Table;
use se_contract::{table, wasm, Generated};
use se_vm::memory::{Slot, STACK_SIZE};
use se_vm::world::Meta;
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod wasm_target;
#[allow(unused_imports)]
pub use wasm_target::require_wasm_target;

/// `game1.wit` run through the generator: one pool, `bodies`.
pub fn game1() -> Generated {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    se_contract::generate(&wit).expect("game1.wit generates")
}

/// The contract table the host reads at run time.
pub fn meta() -> Meta {
    let g = game1();
    let bytes = wasm::section(&g.module, table::SECTION).expect("section");
    Meta::read(&Table::new(bytes).expect("table")).expect("meta")
}


/// Exactly the flags a module must be built with. They are here, in the test,
/// because getting any one of them wrong is a silent failure at run time:
///   --import-memory   or the module writes into a memory nobody reads
///   --global-base     or two modules' statics land on each other
///   -z stack-size     or a module's stack reaches down into the world
pub fn build(dir: &Path, name: &str, slot: Slot, src: &str) -> Result<Vec<u8>> {
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

pub fn prelude() -> &'static str {
    "#![no_std]\n\
     #[panic_handler]\n\
     fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }\n\
     include!(\"contract.rs\");\n\
     #[inline]\n\
     unsafe fn col(i: u32) -> Column { *((arena::COLUMNS + i * 16) as *const Column) }\n\
     #[inline]\n\
     unsafe fn pool(i: u32) -> Pool { *((arena::POOLS + i * 8) as *const Pool) }\n\
     #[inline]\n\
     unsafe fn row<T>(c: &Column, i: u32) -> *mut T { (c.base + i * c.stride) as *mut T }\n"
}

/// Rust puts a `#[link_section]` static into a wasm custom section verbatim,
/// which is exactly what a declaration is. `#[used]` keeps `--gc-sections`
/// from dropping it: nothing references it, because the host reads it out of
/// the file rather than from inside the module.
pub fn embed(section: &str, bytes: &[u8]) -> String {
    let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
    format!(
        "#[used]\n#[link_section = \"{section}\"]\npub static SECTION_{}: [u8; {}] = [{}];\n",
        section.replace('.', "_").to_uppercase(),
        bytes.len(),
        list.join(",")
    )
}

/// Cargo runs these tests on parallel threads, so a directory keyed only by
/// pid is shared — and rustc's intermediate objects then collide under names
/// it picks from the crate name. One directory per build.
pub fn scratch(what: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "se-vm-{what}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A fresh scratch directory holding `game1.wit`'s generated `contract.rs`,
/// which every module source `include!`s.
pub fn contract_dir(what: &str) -> Result<(Generated, PathBuf)> {
    let g = game1();
    let dir = scratch(what);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("contract.rs"), &g.rust)?;
    Ok((g, dir))
}
