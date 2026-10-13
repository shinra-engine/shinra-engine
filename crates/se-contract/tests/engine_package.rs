//! The engine's own records come from `engine.wit`; a game never restates them.

use std::path::PathBuf;

fn wit(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join(name)
}

/// game1.wit with `from` replaced by `to`, written to a temp file.
fn edited(tag: &str, from: &str, to: &str) -> PathBuf {
    let s = std::fs::read_to_string(wit("game1.wit")).unwrap();
    assert!(s.contains(from), "fixture lost `{from}`");
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("engine_{tag}.wit"));
    std::fs::write(&p, s.replace(from, to)).unwrap();
    p
}

#[test]
fn engine_records_are_injected() {
    let g = se_contract::generate(&wit("game1.wit")).expect("game1.wit generates");
    let want = [
        ("span", 8), ("column", 16), ("pool", 8), ("clock", 16),
        ("input", 24), ("command", 52), ("frame", 16),
    ];
    for (name, size) in want {
        let r = g.contract.records.iter().find(|r| r.name == name).expect(name);
        assert_eq!(r.size, size, "{name}");
    }
}

#[test]
fn a_game_may_not_declare_engine_interfaces() {
    let p = edited(
        "handles",
        "interface components {",
        "interface handles { record span { off: u32, len: u32 } }\n\ninterface components {",
    );
    let e = se_contract::generate(&p).err().expect("refused");
    assert!(format!("{e:#}").contains("shinra:engine"), "{e:#}");
}

#[test]
fn a_game_may_not_redeclare_an_engine_record() {
    let p = edited(
        "clock",
        "  record look ",
        "  record clock { t: f32 }\n  record look ",
    );
    let e = se_contract::generate(&p).err().expect("refused");
    let msg = format!("{e:#}");
    assert!(msg.contains("clock") && msg.contains("shinra:engine"), "{msg}");
}
