//! Which artifacts a source edit makes stale.
//!
//! One unit per thing the host loads: `process.wasm` once for every stage,
//! `<cat>/<name>.wasm` for everything else. Only the game and render the run
//! chose are candidates, so editing a sibling never costs a rebuild. This is a
//! pure function of mtimes: it compiles nothing and touches no Vm.

use crate::build;
use crate::discover::{Category, GameSrc, ModuleSrc};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod apply;
#[cfg(test)]
mod apply_asset;
#[cfg(test)]
mod apply_game;
#[cfg(test)]
mod apply_process;

/// The game and render modules the run loaded; `None` means the first one.
pub struct Selection {
    pub game: Option<String>,
    pub render: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unit {
    /// Every module was compiled against it: not a reload, a reopen.
    Contract,
    Module { category: String, name: String, members: Vec<String>, wasm: PathBuf },
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Is any file under `dir` newer than `than`? Iterative, so depth is free.
fn dir_has_newer(dir: &Path, than: SystemTime) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if mtime(&p).is_some_and(|t| t > than) {
                return true;
            }
        }
    }
    false
}

/// Every input of one chosen module: its own `.rs`, its private folder, and
/// the category's shared folders — including any `.wgsl` they `include_bytes!`.
fn module_stale(cat: &Category, m: &ModuleSrc, wasm: &Path) -> bool {
    let Some(o) = mtime(wasm) else { return true };
    mtime(&m.file).is_none_or(|t| t > o)
        || dir_has_newer(&cat.dir.join(&m.name), o)
        || cat.shared.iter().any(|s| dir_has_newer(&cat.dir.join(s), o))
}

fn unit(cat: &Category, name: &str, members: Vec<String>, wasm: PathBuf) -> Unit {
    Unit::Module { category: cat.name.clone(), name: name.to_string(), members, wasm }
}

pub fn stale(src: &GameSrc, out: &Path, sel: &Selection) -> Result<Vec<Unit>> {
    let contract_wasm = out.join("contract.wasm");
    match mtime(&contract_wasm) {
        Some(o) if mtime(&src.contract).is_some_and(|t| t <= o) => {}
        _ => return Ok(vec![Unit::Contract]),
    }
    let mut units = Vec::new();
    for cat in &src.categories {
        if cat.modules.is_empty() {
            continue;
        }
        let cdir = out.join(&cat.name);
        if build::is_additive(&cat.name) {
            let wasm = cdir.join(format!("{}.wasm", cat.name));
            let dirty = mtime(&wasm).is_none_or(|o| dir_has_newer(&cat.dir, o));
            if dirty {
                let members = cat.modules.iter().map(|m| m.name.clone()).collect();
                units.push(unit(cat, &cat.name, members, wasm));
            }
            continue;
        }
        let want = match cat.name.as_str() {
            "game" => Some(sel.game.as_deref()),
            "render" => Some(sel.render.as_deref()),
            _ => None,
        };
        let chosen: Vec<&ModuleSrc> = match want {
            Some(Some(n)) => cat.modules.iter().filter(|m| m.name == n).collect(),
            Some(None) => cat.modules.first().into_iter().collect(),
            None => cat.modules.iter().collect(),
        };
        for m in chosen {
            let wasm = cdir.join(format!("{}.wasm", m.name));
            if module_stale(cat, m, &wasm) {
                units.push(unit(cat, &m.name, vec![m.name.clone()], wasm));
            }
        }
    }
    Ok(units)
}

/// What one poll found: nothing, a contract change, or modules to rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Nothing,
    /// The contract moved: every module and the world layout with it.
    Restart,
    Modules(Vec<Unit>),
}

/// Owns what a poll needs between frames: where the game is, where its
/// outputs are, and which game and render the run chose.
pub struct Reloader {
    root: PathBuf,
    out: PathBuf,
    sel: Selection,
    profile: String,
}

impl Reloader {
    pub fn new(root: PathBuf, out: PathBuf, sel: Selection, profile: String) -> Self {
        Reloader { root, out, sel, profile }
    }

    /// Re-reads the tree each call, so a module created after start is seen.
    /// Reads mtimes only: no rustc, no Vm, no writes.
    pub fn plan(&self) -> Result<Plan> {
        let src = GameSrc::open(&self.root)?;
        let units = stale(&src, &self.out, &self.sel)?;
        Ok(match units.as_slice() {
            [] => Plan::Nothing,
            [Unit::Contract] => Plan::Restart,
            _ => Plan::Modules(units),
        })
    }
}

#[cfg(test)]
mod plan {
    use super::tests::{at, fixture, T0};
    use super::*;

    fn reloader(root: &Path, out: &Path) -> Reloader {
        let sel = Selection { game: None, render: Some("solid".into()) };
        Reloader::new(root.into(), out.into(), sel, "debug".into())
    }

    fn mtimes(dirs: &[&Path]) -> Vec<(PathBuf, Option<SystemTime>)> {
        let mut v = Vec::new();
        let mut stack: Vec<PathBuf> = dirs.iter().map(|d| d.to_path_buf()).collect();
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    v.push((p.clone(), mtime(&p)));
                }
            }
        }
        v.sort();
        v
    }

    #[test]
    fn untouched_tree_is_nothing() {
        let (r, o) = fixture("plan-none");
        assert_eq!(reloader(&r, &o).plan().unwrap(), Plan::Nothing);
    }

    #[test]
    fn an_edited_game_module_is_one_unit() {
        let (r, o) = fixture("plan-game");
        at(&r.join("game/a.rs"), T0 + 20);
        let Plan::Modules(u) = reloader(&r, &o).plan().unwrap() else { panic!("not Modules") };
        assert_eq!(u.len(), 1);
        assert!(matches!(&u[0], Unit::Module { category, name, .. } if category == "game" && name == "a"));
    }

    #[test]
    fn a_newer_contract_is_a_restart() {
        let (r, o) = fixture("plan-contract");
        at(&r.join("contract.wit"), T0 + 20);
        at(&r.join("game/a.rs"), T0 + 20);
        assert_eq!(reloader(&r, &o).plan().unwrap(), Plan::Restart);
    }

    #[test]
    fn a_module_created_after_new_is_seen() {
        let (r, o) = fixture("plan-new");
        let rl = reloader(&r, &o);
        assert_eq!(rl.plan().unwrap(), Plan::Nothing);
        at(&r.join("asset/kettle.rs"), T0 + 20);
        let Plan::Modules(u) = rl.plan().unwrap() else { panic!("not Modules") };
        assert!(matches!(&u[..], [Unit::Module { name, .. }] if name == "kettle"));
    }

    #[test]
    fn planning_twice_writes_nothing() {
        let (r, o) = fixture("plan-twice");
        let rl = reloader(&r, &o);
        let before = mtimes(&[&r, &o]);
        assert_eq!(rl.plan().unwrap(), Plan::Nothing);
        assert_eq!(rl.plan().unwrap(), Plan::Nothing);
        assert_eq!(mtimes(&[&r, &o]), before);
    }
}

#[cfg(test)]
mod tests {
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
}
