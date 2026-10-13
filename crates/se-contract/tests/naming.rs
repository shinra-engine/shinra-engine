//! Generated constants must be valid Rust identifiers for any WIT record
//! name. WIT names are kebab-case, so every constant goes through `snake`.

use std::path::PathBuf;

const WIT: &str = r#"package shinra:naming@0.1.0;

interface components {
  record on-ground { v: u32 }
  record sprite { x: f32 }
}

interface pools {
  use components.{on-ground, sprite};

  record enemy-wave { g: on-ground, s: sprite }
}

interface control {
  record run-state { mode: u32 }
}
"#;

#[test]
fn multi_word_records_emit_valid_constants() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let wit = dir.join("naming.wit");
    std::fs::write(&wit, WIT).expect("write temp wit");
    let g = se_contract::generate(&wit).expect("naming.wit generates");

    for want in ["COL_ON_GROUND", "COL_SPRITE", "POOL_ENEMY_WAVE", "RUN_STATE"] {
        assert!(g.rust.contains(want), "missing {want} in:\n{}", g.rust);
    }
    for line in g.rust.lines().filter(|l| l.trim_start().starts_with("pub const")) {
        let name = line.split(':').next().unwrap();
        assert!(!name.contains('-'), "invalid identifier: {line}");
    }
}
