//! The stale planner against a synthetic tree of mtimes.

use super::*;
use std::fs::{self, File};
use std::time::Duration;

pub(super) const T0: u64 = 1_000_000;

pub(super) fn at(p: &Path, secs: u64) {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).unwrap();
    }
    if !p.exists() {
        fs::write(p, b"x").unwrap();
    }
    let f = File::options().write(true).open(p).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
}

/// A game tree and its outputs, every output newer than every source.
pub(super) fn fixture(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("se-reload-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let root = base.join("g");
    let out = base.join("out");
    for f in [
        "contract.wit",
        "game/a.rs",
        "game/b.rs",
        "game/a/x.txt",
        "render/solid.rs",
        "render/ghost.rs",
        "render/shared/s.wgsl",
        "process/p1.rs",
        "process/p2.rs",
        "asset/bunny.rs",
        "asset/teapot.rs",
    ] {
        at(&root.join(f), T0);
    }
    for f in [
        "contract.wasm",
        "game/a.wasm",
        "game/b.wasm",
        "render/solid.wasm",
        "render/ghost.wasm",
        "process/process.wasm",
        "asset/bunny.wasm",
        "asset/teapot.wasm",
    ] {
        at(&out.join(f), T0 + 10);
    }
    (root, out)
}

/// The render is always `solid`: the default would be `ghost`, the first.
fn plan(root: &Path, out: &Path, game: Option<&str>) -> Vec<Unit> {
    let src = GameSrc::open(root).unwrap();
    let sel = Selection { game: game.map(String::from), render: Some("solid".into()) };
    stale(&src, out, &sel).unwrap()
}

fn names(u: &[Unit]) -> Vec<String> {
    u.iter()
        .map(|u| match u {
            Unit::Contract => "contract".into(),
            Unit::Module { category, name, .. } => format!("{category}/{name}"),
        })
        .collect()
}

#[test]
fn nothing_stale_is_empty() {
    let (r, o) = fixture("none");
    assert!(plan(&r, &o, None).is_empty());
}

#[test]
fn private_file_marks_only_its_selected_module() {
    let (r, o) = fixture("private");
    at(&r.join("game/a/x.txt"), T0 + 20);
    assert_eq!(names(&plan(&r, &o, None)), ["game/a"]);
    assert!(plan(&r, &o, Some("b")).is_empty());
}

#[test]
fn unselected_render_edit_is_ignored() {
    let (r, o) = fixture("ghost");
    at(&r.join("render/ghost.rs"), T0 + 20);
    assert!(plan(&r, &o, None).is_empty());
}

#[test]
fn shared_folder_marks_the_selected_render() {
    let (r, o) = fixture("shared");
    at(&r.join("render/shared/s.wgsl"), T0 + 20);
    assert_eq!(names(&plan(&r, &o, None)), ["render/solid"]);
}

#[test]
fn process_is_one_unit_with_every_member() {
    let (r, o) = fixture("process");
    at(&r.join("process/p2.rs"), T0 + 20);
    let u = plan(&r, &o, None);
    assert_eq!(
        u,
        [Unit::Module {
            category: "process".into(),
            name: "process".into(),
            members: vec!["p1".into(), "p2".into()],
            wasm: o.join("process/process.wasm"),
        }]
    );
}

#[test]
fn a_newer_contract_is_the_only_unit() {
    let (r, o) = fixture("contract");
    at(&r.join("contract.wit"), T0 + 20);
    at(&r.join("game/a.rs"), T0 + 20);
    assert_eq!(plan(&r, &o, None), [Unit::Contract]);
}

#[test]
fn a_missing_wasm_is_stale() {
    let (r, o) = fixture("missing");
    fs::remove_file(o.join("asset/teapot.wasm")).unwrap();
    assert_eq!(names(&plan(&r, &o, None)), ["asset/teapot"]);
}

#[test]
fn selecting_b_ignores_edits_to_a() {
    let (r, o) = fixture("select");
    at(&r.join("game/a.rs"), T0 + 20);
    assert!(plan(&r, &o, Some("b")).is_empty());
    assert_eq!(names(&plan(&r, &o, Some("a"))), ["game/a"]);
}
