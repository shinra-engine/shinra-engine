//! A process swap whose first frame traps is rolled back like any other, but
//! the process slot carries two things the others do not: the stage list the
//! module declared, and statics its stages read. Both must come back.

mod common;

use anyhow::Result;
use se_vm::memory::Slot;
use se_vm::stages::{encode, SECTION, WRITE};
use se_vm::Vm;
use std::path::Path;

/// Every version is this module: the same single static `LUT`, at the same
/// address under the process slot's `--global-base`, and stage `s` (plus `t`
/// when it is declared). `{LUT}` fills the static; `{S}` and `{T}` are the
/// stage bodies, run after `LUT[0]` is read into `x`.
const TEMPLATE: &str = r#"
#[no_mangle]
pub static LUT: [u32; 4] = [{LUT}; 4];

#[no_mangle]
pub extern "C" fn se_stage_run__s() -> u32 {
    unsafe {
        let x = core::ptr::read_volatile(&LUT[0]);
        {S}
        let ct = col(arena::COL_TRANSFORM);
        (&mut *row::<Transform>(&ct, 0)).pos[0] = x as f32;
        1
    }
}

#[no_mangle]
pub extern "C" fn se_stage_run__t() -> u32 {
    {T}
    1
}
"#;

const WRITES: &str = "";
const TRAPS: &str = "core::arch::wasm32::unreachable();";
/// Reads `LUT` first, so the trap comes after the bad statics were used.
const TRAPS_AFTER_READ: &str = "if x != 0 { core::arch::wasm32::unreachable(); }";

/// v1: `LUT` is 1, `s` writes it to `transform` row 0.
fn v1(dir: &Path) -> Result<Vec<u8>> {
    process(dir, "v1", &["s"], 1, WRITES, "")
}

fn process(dir: &Path, name: &str, stages: &[&str], lut: u32, s: &str, t: &str) -> Result<Vec<u8>> {
    let table: Vec<u8> = stages.iter().flat_map(|n| encode(n, se_vm::stages::ROW, &[("transform", WRITE, 0)])).collect();
    let body = TEMPLATE
        .replace("{LUT}", &format!("{lut:#x}"))
        .replace("{S}", s)
        .replace("{T}", t);
    let src = format!("{}{}{body}", common::prelude(), common::embed(SECTION, &table));
    common::build(dir, name, Slot::Process, &src)
}

/// A VM running v1 for one frame, so v1 is what a rollback goes back to.
fn running(what: &str) -> Result<(Vm, std::path::PathBuf)> {
    let (g, dir) = common::contract_dir(what)?;
    let mut vm = Vm::new(&g.module, 16)?;
    vm.load(Slot::Process, "v1", v1(&dir)?)?;
    vm.frame(1)?;
    assert_eq!(pos_x(&vm)?, 1.0);
    Ok((vm, dir))
}

fn zero_pos_x(vm: &mut Vm) -> Result<()> {
    let t = vm.meta.column_index("transform").expect("transform");
    vm.write(vm.world.row_at(t, 0), &0f32.to_le_bytes())
}

fn pos_x(vm: &Vm) -> Result<f32> {
    let t = vm.meta.column_index("transform").expect("transform");
    Ok(f32::from_le_bytes(vm.read(vm.world.row_at(t, 0), 4)?.try_into().unwrap()))
}

fn names(vm: &Vm) -> Vec<String> {
    vm.stages().iter().map(|s| s.name.clone()).collect()
}

/// The swap traps; the frame says so, and the next one is v1's.
fn rolled_back(vm: &mut Vm) -> Result<()> {
    let err = format!("{:#}", vm.frame(2).unwrap_err());
    assert!(err.contains("rolled back"), "{err}");
    assert_eq!(vm.take_rollback(), Some(vec![Slot::Process]));
    assert_eq!(vm.module(Slot::Process).map(|m| m.name.as_str()), Some("v1"));
    zero_pos_x(vm)?;
    vm.frame(3)?;
    Ok(())
}

#[test]
fn j_a_trapping_process_is_rolled_back_to_the_previous_one() -> Result<()> {
    common::require_wasm_target();
    let (mut vm, dir) = running("swap-trapprocess")?;
    vm.load(Slot::Process, "v2", process(&dir, "v2", &["s"], 1, TRAPS, "")?)?;

    rolled_back(&mut vm)?;
    assert_eq!(names(&vm), ["s"]);
    assert_eq!(pos_x(&vm)?, 1.0, "v1's stage writes LUT[0] again");
    Ok(())
}

/// v2's data segment wrote 0xFFFF_FFFF over `LUT` when it was instantiated.
/// Putting v1's old instance back would leave that there; re-instantiating
/// v1 writes its own `LUT` back.
#[test]
fn k_statics_are_restored_by_the_rollback() -> Result<()> {
    common::require_wasm_target();
    let (mut vm, dir) = running("swap-trapstatics")?;
    let v2 = process(&dir, "v2", &["s"], 0xFFFF_FFFF, TRAPS_AFTER_READ, "")?;
    vm.load(Slot::Process, "v2", v2)?;

    rolled_back(&mut vm)?;
    assert_eq!(pos_x(&vm)?, 1.0, "v1's LUT, not v2's 0xFFFF_FFFF");
    Ok(())
}

#[test]
fn l_the_stage_list_is_restored_by_the_rollback() -> Result<()> {
    common::require_wasm_target();
    let (mut vm, dir) = running("swap-trapstages")?;
    vm.load(Slot::Process, "v2", process(&dir, "v2", &["s", "t"], 1, WRITES, TRAPS)?)?;
    assert_eq!(names(&vm), ["s", "t"]);

    rolled_back(&mut vm)?;
    assert_eq!(names(&vm), ["s"]);
    assert_eq!(pos_x(&vm)?, 1.0);
    Ok(())
}

#[test]
fn m_a_process_swap_that_runs_keeps_its_stages() -> Result<()> {
    common::require_wasm_target();
    let (mut vm, dir) = running("swap-goodstages")?;
    vm.load(Slot::Process, "v2", process(&dir, "v2", &["s", "t"], 2, WRITES, "")?)?;
    vm.frame(2)?;
    assert_eq!(vm.take_rollback(), None);
    assert_eq!(names(&vm), ["s", "t"]);
    assert_eq!(pos_x(&vm)?, 2.0, "v2's stage and v2's LUT");
    Ok(())
}
