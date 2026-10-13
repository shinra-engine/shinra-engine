//! `#[se::pair]` built through the real CLI and run on the VM.
//!
//! One pool pairs with itself (each unordered pair once), two pools pair
//! across (every i × j).

use se_vm::memory::Slot;
use se_vm::Vm;
use std::path::{Path, PathBuf};
use std::process::Command;

const CONTRACT: &str = r#"package shinra:pairs@0.1.0;

interface components {
  record acc { v: f32 }
  record mark { n: f32 }
}

interface pools {
  use components.{acc, mark};

  record bodies { a: acc }
  record marks { m: mark }
}

interface control {
  record look { bodies: u32, marks: u32 }
}
"#;

const GAME: &str = r#"use crate::*;

#[no_mangle]
pub extern "C" fn tick(_frame: u32) -> u32 {
    unsafe {
        (*se_pool_mut(arena::POOL_BODIES)).len = 4;
        (*se_pool_mut(arena::POOL_MARKS)).len = 3;
    }
    0
}
"#;

const RENDER: &str = r#"use crate::*;

#[no_mangle]
pub extern "C" fn record() -> u32 {
    0
}
"#;

const HIT: &str = r#"use crate::*;

#[se::pair]
fn meet((a,): (&mut Acc,), (b,): (&mut Acc,)) {
    a.v += 1.0;
    b.v += 1.0;
}

#[se::pair]
fn tally((_a,): (&Acc,), (m,): (&mut Mark,)) {
    m.n += 1.0;
}
"#;

fn has_wasm_target() -> bool {
    let Ok(o) = Command::new("rustc")
        .args(["--print", "target-libdir", "--target", "wasm32-unknown-unknown"])
        .output()
    else {
        return false;
    };
    o.status.success() && Path::new(String::from_utf8_lossy(&o.stdout).trim()).is_dir()
}

fn put(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn column(vm: &Vm, comp: &str, rows: u32) -> Vec<f32> {
    let c = vm.meta.column_index(comp).unwrap();
    (0..rows)
        .map(|i| {
            let b = vm.read(vm.world.row_at(c, i), 4).unwrap();
            f32::from_le_bytes([b[0], b[1], b[2], b[3]])
        })
        .collect()
}

#[test]
fn pair_stages_visit_each_pair() {
    if !has_wasm_target() {
        eprintln!("skipped: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir: PathBuf = std::env::temp_dir().join(format!("shinra-pair-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let game = dir.join("game");
    put(&game, "contract.wit", CONTRACT);
    put(&game, "game/g.rs", GAME);
    put(&game, "render/r.rs", RENDER);
    put(&game, "process/hit.rs", HIT);
    let out = dir.join("out");
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let run = Command::new(env!("CARGO_BIN_EXE_shinra"))
        .arg("build")
        .arg(&game)
        .arg("-o")
        .arg(&out)
        .env("SHINRA_ENGINE", &engine)
        .output()
        .unwrap();
    assert!(run.status.success(), "shinra build failed:\n{}", String::from_utf8_lossy(&run.stderr));

    let rd = |p: &str| std::fs::read(out.join(p)).unwrap();
    let mut vm = Vm::new(&rd("contract.wasm"), 8).unwrap();
    vm.load(Slot::Game, "g", rd("game/g.wasm")).unwrap();
    vm.load(Slot::Process, "process", rd("process/process.wasm")).unwrap();
    vm.load(Slot::Render, "r", rd("render/r.wasm")).unwrap();
    assert!(vm.stages().iter().all(|s| s.kind == se_vm::stages::PAIR));
    vm.frame(0).unwrap();

    // Four bodies pair with each other: every row meets the other three.
    assert_eq!(column(&vm, "acc", 4), [3.0; 4]);
    // Three marks each see all four bodies.
    assert_eq!(column(&vm, "mark", 3), [4.0; 3]);

    let _ = std::fs::remove_dir_all(&dir);
}
