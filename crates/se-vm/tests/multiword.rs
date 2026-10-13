//! A multi-word component resolves: the stage macro writes the WIT (kebab)
//! name into `se.stages`, and `resolve` finds it in the contract table.

use se_vm::stages::{self, WRITE};
use se_vm::Vm;

const WIT: &str = r#"package shinra:multiword@0.1.0;

interface components {
  record transform { x: f32, y: f32 }
  record on-ground { height: f32, grounded: u32 }
}

interface pools {
  use components.{transform, on-ground};
  record walkers { t: transform, g: on-ground }
}

interface control {
  record look { asset: u32, graph: u32, bodies: u32 }
}
"#;

#[test]
fn on_ground_resolves_to_its_column() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("se-vm-multiword-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let wit = dir.join("contract.wit");
    std::fs::write(&wit, WIT)?;
    let g = se_contract::generate(&wit)?;
    assert!(g.rust.contains("COL_ON_GROUND"), "emitter must name the column COL_ON_GROUND");

    let vm = Vm::new(&g.module, 8)?;
    let mut ss = stages::decode(&stages::encode("f", stages::ROW, &[("on-ground", WRITE, 0)]))?;
    stages::resolve(&mut ss, &vm.meta)?;

    let want = vm.meta.components.iter().position(|r| r.name == "on-ground").unwrap() as u32;
    assert_eq!(ss[0].params[0].column, want);
    Ok(())
}
