//! A module that never returns costs the frame, not the process.
//!
//! Each runaway is a module built here, the way `frame.rs` builds its own.

mod common;
use common::require_wasm_target;

use anyhow::{bail, Result};
use se_vm::memory::{Slot, STACK_SIZE};
use se_vm::module::DEFAULT_FUEL;
use se_vm::Vm;
use std::path::{Path, PathBuf};
use std::process::Command;


/// Exactly the flags a module must be built with. They are here, in the test,
/// because getting any one of them wrong is a silent failure at run time:
///   --import-memory   or the module writes into a memory nobody reads
///   --global-base     or two modules' statics land on each other
///   -z stack-size     or a module's stack reaches down into the world
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

fn prelude() -> &'static str {
    "#![no_std]\n\
     #[panic_handler]\n\
     fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }\n\
     include!(\"contract.rs\");\n\
     #[used]\n#[link_section = \"se.contract-id\"]\npub static SE_CONTRACT_ID: [u8; 8] = CONTRACT_ID.to_le_bytes();\n\
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
fn embed(section: &str, bytes: &[u8]) -> String {
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
fn scratch(what: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "se-vm-{what}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

const SPIN: &str = "loop { unsafe { core::ptr::read_volatile(&0u32) }; }";

fn stage_section() -> Vec<u8> {
    se_vm::stages::encode("motion", se_vm::stages::ROW, &[("orbit", se_vm::stages::READ, 0), ("transform", se_vm::stages::WRITE, 0)])
}

const HEALTHY_GAME: &str = r#"
#[no_mangle]
pub extern "C" fn tick(_n: u32) -> u32 { 0 }
"#;

/// Start function `loop {}`, importing only `env::memory`: it runs inside
/// instantiation, before any export can be called. It also exports a harmless
/// `tick(u32) -> u32`, because a game module without one is refused at load,
/// before instantiation, and the budget would never be what stops it.
const SPINNING_START: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
    0x01, 0x09, 0x02, 0x60, 0x00, 0x00, 0x60, 0x01, 0x7f, 0x01, 0x7f, // types: () -> (), (i32) -> i32
    0x02, 0x0f, 0x01, 0x03, b'e', b'n', b'v', 0x06, b'm', b'e', b'm', b'o', b'r', b'y', 0x02,
    0x00, 0x01, // import env.memory, min 1
    0x03, 0x03, 0x02, 0x00, 0x01, // two functions: the start loop, and tick
    0x07, 0x08, 0x01, 0x04, b't', b'i', b'c', b'k', 0x00, 0x01, // export tick = function 1
    0x08, 0x01, 0x00, // start = function 0
    0x0a, 0x0e, 0x02, 0x07, 0x00, 0x03, 0x40, 0x0c, 0x00, 0x0b, 0x0b, // loop { br 0 }
    0x04, 0x00, 0x41, 0x00, 0x0b, // tick: i32.const 0
];

fn contract(dir: &Path) -> Result<Vec<u8>> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("contract.rs"), &g.rust)?;
    Ok(g.module)
}

fn chain(e: &anyhow::Error) -> String {
    format!("{e:#}")
}

#[test]
fn a_spinning_stage_fails_the_frame_and_names_itself() -> Result<()> {
    require_wasm_target();
    let dir = scratch("runaway-stage");
    let contract = contract(&dir)?;
    let section = embed(se_vm::stages::SECTION, &stage_section());
    let src = format!(
        "{}{section}#[no_mangle] pub extern \"C\" fn se_stage_run__motion() -> u32 {{ {SPIN} }}\n",
        prelude()
    );
    let mut vm = Vm::new(&contract, 16)?;
    vm.load(Slot::Process, "process", build(&dir, "process", Slot::Process, &src)?)?;
    let err = chain(&vm.frame(1).unwrap_err());
    assert!(err.contains("stage `motion`") && err.contains("budget"), "{err}");
    Ok(())
}

#[test]
fn a_looping_start_function_fails_the_load() -> Result<()> {
    require_wasm_target();
    let contract = contract(&scratch("runaway-start"))?;
    let mut vm = Vm::new(&contract, 16)?;
    // A custom section carrying this world's contract id, appended to the file.
    let mut bytes = SPINNING_START.to_vec();
    let name = se_contract::table::CONTRACT_ID_SECTION;
    bytes.extend([0x00, (1 + name.len() + 8) as u8, name.len() as u8]);
    bytes.extend(name.as_bytes());
    bytes.extend(vm.contract_id().to_le_bytes());
    let err = chain(&vm.load(Slot::Game, "loop", bytes).unwrap_err());
    assert!(err.contains("instantiat") && err.contains("budget"), "{err}");
    Ok(())
}

#[test]
fn a_healthy_game_recovers_the_vm() -> Result<()> {
    require_wasm_target();
    let dir = scratch("runaway-recover");
    let contract = contract(&dir)?;
    let spin = format!(
        "{}#[no_mangle] pub extern \"C\" fn tick(_n: u32) -> u32 {{ {SPIN} }}\n",
        prelude()
    );
    let mut vm = Vm::new(&contract, 16)?;
    vm.load(Slot::Game, "spin", build(&dir, "spin", Slot::Game, &spin)?)?;

    let orbit = vm.meta.column_index("orbit").unwrap();
    let stride = vm.world.stride[orbit as usize] as usize;
    let mut seeded = Vec::new();
    for i in 0..4u32 {
        let mut bytes: Vec<u8> = (0..stride as u32).map(|b| (b + i) as u8).collect();
        bytes[0] = i as u8 + 1;
        vm.write(vm.world.row_at(orbit, i), &bytes)?;
        seeded.push(bytes);
    }

    let err = chain(&vm.frame(1).unwrap_err());
    assert!(err.contains("budget") && err.contains("tick"), "{err}");

    let healthy = format!("{}{HEALTHY_GAME}", prelude());
    vm.load(Slot::Game, "healthy", build(&dir, "healthy", Slot::Game, &healthy)?)?;
    vm.frame(1)?;
    for (i, want) in seeded.iter().enumerate() {
        let back = vm.read(vm.world.row_at(orbit, i as u32), stride as u32)?;
        assert_eq!(&back, want, "row {i} survived the runaway");
    }
    Ok(())
}

/// The budget is a measurement, not a guess: a healthy tick must leave it
/// almost untouched, or `DEFAULT_FUEL` is in the way of real games.
#[test]
fn a_healthy_frame_uses_a_small_fraction_of_the_budget() -> Result<()> {
    require_wasm_target();
    let dir = scratch("runaway-measure");
    let contract = contract(&dir)?;
    let healthy = format!("{}{HEALTHY_GAME}", prelude());
    let mut vm = Vm::new(&contract, 16)?;
    // Only the game slot is filled, so the frame makes one entry: the tick.
    vm.load(Slot::Game, "healthy", build(&dir, "healthy", Slot::Game, &healthy)?)?;
    vm.frame(1)?;
    let used = DEFAULT_FUEL - vm.store.get_fuel()?;
    println!("fuel used: {used} of {DEFAULT_FUEL}");
    assert!(used > 0, "the tick ran but spent no fuel");
    assert!(used < DEFAULT_FUEL / 100, "a healthy tick used {used} of {DEFAULT_FUEL}");
    Ok(())
}
