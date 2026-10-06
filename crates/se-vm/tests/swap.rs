//! A swap is refused before it can touch anything: a game or render module
//! without its entry point never gets an instance, and the one it would have
//! replaced keeps running.

mod common;
use common::require_wasm_target;

use anyhow::{bail, Result};
use se_vm::memory::{Slot, STACK_SIZE};
use se_vm::Vm;
use std::path::{Path, PathBuf};
use std::process::Command;


/// The flags of frame.rs `build`; see there for why each one matters.
fn build(dir: &Path, name: &str, slot: Slot, src: &str) -> Result<Vec<u8>> {
    let file = dir.join(format!("{name}.rs"));
    std::fs::write(&file, format!("{}{src}", prelude()))?;
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
     #[inline]\n\
     unsafe fn pool(i: u32) -> Pool { *((arena::POOLS + i * 8) as *const Pool) }\n"
}

const GAME: &str = r#"
#[no_mangle]
pub extern "C" fn tick(n: u32) -> u32 {
    unsafe {
        let look = &mut *(arena::LOOK as *mut Look);
        look.bodies = 2 + n % 4;
        look.asset = n % 2;
        look.graph = n / 2 % 2;
        let cam = &mut *(arena::CAMERA as *mut Camera);
        cam.eye = [0.0, 1.5, 6.0];
        cam.fov = 0.9;
        cam.yaw = n as f32 * 0.25;
        0
    }
}
"#;

const RENDER: &str = r#"
#[no_mangle]
pub extern "C" fn record() -> u32 {
    unsafe {
        let look = &*(arena::LOOK as *const Look);
        let n = look.bodies.min(pool(arena::POOL_BODIES).cap);
        let zero = Command {
            kind: 0, vertices: 0, shader: 0, column: u32::MAX, count: 0,
            uniform_off: 0, uniform_len: 0, pad: 0,
            clear_r: 0.0, clear_g: 0.0, clear_b: 0.0, clear_a: 1.0,
        };
        let cmds = arena::COMMANDS as *mut Command;
        *cmds.add(0) = Command { kind: 0, ..zero };
        *cmds.add(1) = Command {
            kind: 1, column: arena::COL_TRANSFORM, count: n,
            uniform_off: arena::CAMERA, uniform_len: Camera::SIZE, ..zero
        };
        *cmds.add(2) = Command { kind: 3, ..zero };
        let f = &mut *(arena::FRAME as *mut Frame);
        f.commands = arena::COMMANDS;
        f.count = 3;
        f.present = 0;
        f.epoch = f.epoch.wrapping_add(1);
        3
    }
}
"#;

/// One directory per test: they run on parallel threads and rustc's
/// intermediates would collide.
fn setup(what: &str) -> Result<(Vm, PathBuf)> {
    let g = common::game1();
    let dir = std::env::temp_dir().join(format!("se-vm-swap-{what}-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("contract.rs"), &g.rust)?;
    let mut vm = Vm::new(&g.module, 16)?;
    vm.load(Slot::Game, "game", build(&dir, "game", Slot::Game, GAME)?)?;
    vm.load(Slot::Render, "render", build(&dir, "render", Slot::Render, RENDER)?)?;
    Ok((vm, dir))
}

fn bodies(vm: &Vm) -> Result<u32> {
    let (at, _) = vm.meta.control_at("look").expect("game1 declares look");
    let r = vm.meta.record("look").expect("look is a record");
    let off = r.fields.iter().find(|f| f.name == "bodies").expect("look.bodies").offset;
    Ok(u32::from_le_bytes(vm.read(at + off, 4)?.try_into().unwrap()))
}

#[test]
fn a_game_without_tick_is_refused_and_the_old_one_keeps_ticking() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("notick")?;
    let src = "#[no_mangle] pub extern \"C\" fn tock(n: u32) -> u32 { n }\n";
    let bytes = build(&dir, "game2", Slot::Game, src)?;
    let err = vm.load(Slot::Game, "game2", bytes).unwrap_err().to_string();
    assert!(err.contains("tick"), "the refusal names the missing export: {err}");

    vm.frame(1)?;
    assert_eq!(bodies(&vm)?, 3, "the previous game still ticks: 2 + 1 % 4");
    Ok(())
}

#[test]
fn a_render_without_record_is_refused_and_the_old_one_keeps_recording() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("norecord")?;
    let src = "#[no_mangle] pub extern \"C\" fn draw() -> u32 { 0 }\n";
    let bytes = build(&dir, "render2", Slot::Render, src)?;
    let err = vm.load(Slot::Render, "render2", bytes).unwrap_err().to_string();
    assert!(err.contains("record"), "the refusal names the missing export: {err}");

    let (_, cmds) = vm.frame(1)?;
    assert_eq!(cmds, 3, "render v1 still records");
    Ok(())
}

#[test]
fn a_tick_of_the_wrong_type_is_refused() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("badtick")?;
    let src = "#[no_mangle] pub extern \"C\" fn tick() -> u32 { 0 }\n";
    let bytes = build(&dir, "game3", Slot::Game, src)?;
    let err = vm.load(Slot::Game, "game3", bytes).unwrap_err().to_string();
    assert!(err.contains("tick"), "{err}");
    vm.frame(1)?;
    assert_eq!(bodies(&vm)?, 3);
    Ok(())
}

/// A process module declaring one stage, `s`, that writes `transform`.
fn process(dir: &Path, name: &str, body: &str) -> Result<Vec<u8>> {
    use se_vm::stages::{encode, SECTION, WRITE};
    let section = common::embed(SECTION, &encode("s", &[("transform", WRITE)]));
    let src = format!("{}{section}{body}", common::prelude());
    common::build(dir, name, Slot::Process, &src)
}

/// Both versions define the same statics, so at the process slot's
/// `--global-base` v2's `LUT` lands exactly on v1's.
const GOOD: &str = r#"
#[no_mangle]
pub static LUT: [u32; 4] = [1, 2, 3, 4];

#[no_mangle]
pub extern "C" fn se_stage_run__s() -> u32 {
    unsafe {
        let ct = col(arena::COL_TRANSFORM);
        let x = core::ptr::read_volatile(&LUT[0]) as f32;
        (&mut *row::<Transform>(&ct, 0)).pos[0] = x;
        1
    }
}
"#;

/// Declares `s`, exports no `se_stage_run__s`.
const BAD: &str = r#"
#[no_mangle]
pub static LUT: [u32; 4] = [0xFFFF_FFFF; 4];
"#;

fn pos_x(vm: &Vm) -> Result<f32> {
    let t = vm.meta.column_index("transform").expect("transform");
    Ok(f32::from_le_bytes(vm.read(vm.world.row_at(t, 0), 4)?.try_into().unwrap()))
}

fn names(vm: &Vm) -> Vec<String> {
    vm.stages().iter().map(|s| s.name.clone()).collect()
}

#[test]
fn d_a_stage_without_its_export_is_refused_and_the_old_stages_run() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("noexport")?;
    vm.load(Slot::Process, "p1", process(&dir, "p1", GOOD)?)?;
    let bad = "#[no_mangle] pub extern \"C\" fn se_stage_run__other() -> u32 { 0 }\n";
    let err = vm.load(Slot::Process, "p2", process(&dir, "p2", bad)?).unwrap_err().to_string();
    assert!(err.contains("se_stage_run__s"), "the refusal names the missing export: {err}");

    assert_eq!(names(&vm), ["s"]);
    let t = vm.meta.column_index("transform").expect("transform");
    vm.write(vm.world.row_at(t, 0), &0f32.to_le_bytes())?;
    vm.frame(1)?;
    assert_eq!(pos_x(&vm)?, 1.0, "p1's stage still writes the transform column");
    Ok(())
}

/// Fails on the instantiate-then-check order: v2's data segment would have
/// overwritten `LUT` with 0xFFFF_FFFF before the refusal (seen: pos.x came
/// back 4294967300.0, the u32 cast to f32).
#[test]
fn e_statics_survive_a_refusal() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("statics")?;
    vm.load(Slot::Process, "v1", process(&dir, "v1", GOOD)?)?;
    vm.frame(1)?;
    assert_eq!(pos_x(&vm)?, 1.0);

    let t = vm.meta.column_index("transform").expect("transform");
    vm.write(vm.world.row_at(t, 0), &0f32.to_le_bytes())?;
    assert!(vm.load(Slot::Process, "v2", process(&dir, "v2", BAD)?).is_err());

    vm.frame(2)?;
    assert_eq!(pos_x(&vm)?, 1.0, "v1's LUT is intact");
    assert_eq!(names(&vm), ["s"]);
    Ok(())
}

/// Well formed, and traps the first time it runs.
const TRAP_GAME: &str =
    "#[no_mangle] pub extern \"C\" fn tick(_: u32) -> u32 { core::arch::wasm32::unreachable() }\n";

const TRAP_RENDER: &str =
    "#[no_mangle] pub extern \"C\" fn record() -> u32 { core::arch::wasm32::unreachable() }\n";

#[test]
fn f_a_trapping_game_is_rolled_back_to_the_previous_one() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("trapgame")?;
    vm.load(Slot::Game, "game2", build(&dir, "game2", Slot::Game, TRAP_GAME)?)?;

    let err = format!("{:#}", vm.frame(1).unwrap_err());
    assert!(err.contains("rolled back"), "{err}");
    assert_eq!(vm.take_rollback(), Some(vec![Slot::Game]));

    vm.frame(1)?;
    assert_eq!(bodies(&vm)?, 3, "v1 ticks again: 2 + 1 % 4");
    assert_eq!(vm.take_rollback(), None);
    Ok(())
}

/// Nothing ran before it, so there is nothing to go back to: the trapping
/// module stays in its slot.
#[test]
fn g_a_first_load_that_traps_stages_no_rollback() -> Result<()> {
    require_wasm_target();
    let (g, dir) = common::contract_dir("swap-firsttrap")?;
    let mut vm = Vm::new(&g.module, 16)?;
    vm.load(Slot::Game, "game", build(&dir, "game", Slot::Game, TRAP_GAME)?)?;

    let err = format!("{:#}", vm.frame(1).unwrap_err());
    assert!(!err.contains("rolled back"), "{err}");
    assert_eq!(vm.take_rollback(), None);
    assert_eq!(vm.module(Slot::Game).map(|m| m.name.as_str()), Some("game"));
    Ok(())
}

#[test]
fn h_two_swaps_before_a_frame_roll_back_to_the_original() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("twoswaps")?;
    let v2 = "#[no_mangle] pub extern \"C\" fn tick(_: u32) -> u32 { \
              unsafe { (*(arena::LOOK as *mut Look)).bodies = 7; } 0 }\n";
    vm.load(Slot::Game, "game2", build(&dir, "game2", Slot::Game, v2)?)?;
    vm.load(Slot::Game, "game3", build(&dir, "game3", Slot::Game, TRAP_GAME)?)?;

    assert!(vm.frame(1).is_err());
    assert_eq!(vm.take_rollback(), Some(vec![Slot::Game]));
    assert_eq!(vm.module(Slot::Game).map(|m| m.name.as_str()), Some("game"));
    vm.frame(1)?;
    assert_eq!(bodies(&vm)?, 3, "v1, not v2 (which would write 7)");
    Ok(())
}

#[test]
fn i_a_trapping_render_is_rolled_back_by_record_only() -> Result<()> {
    require_wasm_target();
    let (mut vm, dir) = setup("traprender")?;
    vm.frame(1)?;
    vm.load(Slot::Render, "render2", build(&dir, "render2", Slot::Render, TRAP_RENDER)?)?;

    let err = format!("{:#}", vm.record_only().unwrap_err());
    assert!(err.contains("rolled back"), "{err}");
    assert_eq!(vm.take_rollback(), Some(vec![Slot::Render]));

    let (_, cmds) = vm.record_only()?;
    assert_eq!(cmds, 3, "render v1 records again");
    Ok(())
}
