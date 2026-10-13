//! The host writes `control.clock` before every tick; a game only reads it.

mod common;

use anyhow::Result;
use common::{build, contract_dir, prelude, require_wasm_target};
use se_vm::clock::{Clock, STEP};
use se_vm::memory::Slot;
use se_vm::Vm;

/// Copies what the host wrote into control vars the test can read back.
const COPY: &str = r#"
#[no_mangle]
pub extern "C" fn tick(_n: u32) -> u32 {
    unsafe {
        let c = &*(arena::CLOCK as *const Clock);
        let look = &mut *(arena::LOOK as *mut Look);
        look.bodies = c.frame;
        let cam = &mut *(arena::CAMERA as *mut Camera);
        cam.fov = c.t;
        cam.dist = c.dt;
        0
    }
}
"#;

/// Overwrites the clock; the host puts it back before the next tick.
const SCRIBBLE: &str = r#"
#[no_mangle]
pub extern "C" fn tick(_n: u32) -> u32 {
    unsafe { (&mut *(arena::CLOCK as *mut Clock)).t = 999.0; 0 }
}
"#;

fn f32_at(vm: &Vm, name: &str, off: u32) -> Result<f32> {
    let (at, _) = vm.meta.control_at(name).expect(name);
    Ok(f32::from_le_bytes(vm.read(at + off, 4)?.try_into().unwrap()))
}

#[test]
fn the_host_writes_the_clock_before_every_tick() -> Result<()> {
    require_wasm_target();
    let (g, dir) = contract_dir("clock")?;
    let game = build(&dir, "game", Slot::Game, &format!("{}{COPY}", prelude()))?;
    let mut vm = Vm::new(&g.module, 64)?;
    vm.load(Slot::Game, "game", game)?;

    vm.frame(8)?;
    let (look, _) = vm.meta.control_at("look").unwrap();
    let bodies = u32::from_le_bytes(vm.read(look + 8, 4)?.try_into().unwrap());
    // The roster is clamped to capacity (64); the frame number was copied first.
    assert_eq!(bodies, 8);
    let cam = vm.meta.record("camera").unwrap();
    let off = |n: &str| cam.fields.iter().find(|f| f.name == n).unwrap().offset;
    assert!((f32_at(&vm, "camera", off("fov"))? - 8.0 * STEP).abs() < 1e-6);
    assert_eq!(f32_at(&vm, "camera", off("dist"))?, STEP);
    Ok(())
}

#[test]
fn a_game_cannot_keep_the_clock() -> Result<()> {
    require_wasm_target();
    let (g, dir) = contract_dir("clock-scribble")?;
    let game = build(&dir, "game", Slot::Game, &format!("{}{SCRIBBLE}", prelude()))?;
    let mut vm = Vm::new(&g.module, 64)?;
    vm.load(Slot::Game, "game", game)?;
    vm.frame(7)?;
    assert_eq!(f32_at(&vm, "clock", 0)?, 999.0, "the game wrote it during its tick");
    vm.set_clock(Clock::at(9))?;
    assert_eq!(f32_at(&vm, "clock", 0)?, 9.0 * STEP);
    Ok(())
}
