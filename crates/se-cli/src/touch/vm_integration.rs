//! Proves the whole chain end to end, as documented at the `mod vm_integration`
//! declaration in `touch.rs`.

use super::*;
use se_vm::memory::{Slot, STACK_SIZE};
use se_vm::Vm;
use std::process::Command;

/// `pad` is deliberately absent — `input` needs no game-declared control
/// var at all, since the engine package injects it into every contract.
const WIT: &str = "package shinra:touchfix@0.1.0;

interface components {
  record unused { v: f32 }
}

interface pools {
  use components.{unused};
  record unused-pool { u: unused }
}

interface control {
  record look { mark: u32 }
}
";

const GAME: &str = "#![no_std]
#[panic_handler]
fn se_panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }
include!(\"contract.rs\");
#[used]
#[link_section = \"se.contract-id\"]
pub static SE_CONTRACT_ID: [u8; 8] = CONTRACT_ID.to_le_bytes();

#[no_mangle]
pub extern \"C\" fn tick(_n: u32) -> u32 {
    unsafe {
        let input = &*(arena::INPUT as *const Input);
        let look = &mut *(arena::LOOK as *mut Look);
        look.mark = input.mouse_down;
    }
    0
}
";

fn has_wasm_target() -> bool {
    let Ok(o) = Command::new("rustc")
        .args([
            "--print",
            "target-libdir",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .output()
    else {
        return false;
    };
    o.status.success() && Path::new(String::from_utf8_lossy(&o.stdout).trim()).is_dir()
}

/// Builds `game.wasm` the same way `pad.rs`'s `vm_integration` fixture does.
fn build_game(dir: &Path) -> anyhow::Result<Vec<u8>> {
    let src = dir.join("game.rs");
    std::fs::write(&src, GAME)?;
    let wasm = dir.join("game.wasm");
    let out = Command::new("rustc")
        .args(["--edition", "2021", "--target", "wasm32-unknown-unknown"])
        .args(["--crate-type", "cdylib", "--crate-name", "game"])
        .arg("-C")
        .arg("link-arg=--import-memory")
        .arg("-C")
        .arg(format!(
            "link-arg=--global-base={}",
            Slot::Game.global_base()
        ))
        .arg("-C")
        .arg(format!("link-arg=-zstack-size={STACK_SIZE}"))
        .arg("-C")
        .arg("opt-level=1")
        .arg("-C")
        .arg("panic=abort")
        .arg("-o")
        .arg(&wasm)
        .arg(&src)
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "building game.wasm failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(std::fs::read(wasm)?)
}

/// Runs the script through a fresh `Vm` and returns `look.mark` read back
/// after every frame — the chain's one observable output. The synthetic
/// viewport is 10x10, so cell (5, 5) falls inside it.
fn run_script(contract: &[u8], game: &[u8], script: &Path) -> anyhow::Result<Vec<u32>> {
    let mut vm = Vm::new(contract, 1)?;
    vm.load(Slot::Game, "game", game.to_vec())?;
    let mut ts = ScriptedTouch::open(script, (10, 10))?;
    let (at, len) = vm
        .meta
        .control_at("look")
        .expect("look is a declared control var");
    let mut marks = Vec::new();
    for f in 0..5u32 {
        vm.set_input(ts.poll(f))?;
        vm.frame(f)?;
        let b = vm.read(at, len)?;
        marks.push(u32::from_le_bytes(b[0..4].try_into().unwrap()));
    }
    Ok(marks)
}

#[test]
fn scripted_touch_drives_a_real_game_through_set_input_and_frame() -> anyhow::Result<()> {
    if !has_wasm_target() {
        eprintln!("skipped: wasm32-unknown-unknown is not installed");
        return Ok(());
    }
    let dir = std::env::temp_dir().join(format!("se-cli-touch-vm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;

    let wit = dir.join("contract.wit");
    std::fs::write(&wit, WIT)?;
    let g = se_contract::generate(&wit)?;
    std::fs::write(dir.join("contract.rs"), &g.rust)?;
    let game = build_game(&dir)?;

    // (5, 5) is inside the synthetic 10x10 viewport (inner origin (1,1));
    // held on frames 1-2, released by frame 3; frame 0 and the repeated
    // frame 4 are never down.
    let script = dir.join("touch.script");
    std::fs::write(&script, "-\ndown 5,5\ndrag 5,5\nup 5,5\n-\n")?;

    let first = run_script(&g.module, &game, &script)?;
    let second = run_script(&g.module, &game, &script)?;

    assert_eq!(
        first,
        [0, 1, 1, 0, 0],
        "mark must reflect mouse_down on exactly the held-and-inside frames"
    );
    assert_eq!(
        first, second,
        "the same script must produce identical output every run"
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
