//! Which artifacts a source edit makes stale.
//!
//! One unit per thing the host loads: `process.wasm` once for every stage,
//! `<cat>/<name>.wasm` for everything else. Only the game and render the run
//! chose are candidates, so editing a sibling never costs a rebuild. This is a
//! pure function of mtimes: it compiles nothing and touches no Vm.

use crate::build;
use crate::discover::{Category, GameSrc, ModuleSrc};
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod apply;
#[cfg(test)]
mod apply_asset;
#[cfg(test)]
mod apply_game;
#[cfg(test)]
mod apply_process;
#[cfg(test)]
mod failure;
#[cfg(test)]
mod refusal;

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
    mtime(wasm).is_none_or(|o| module_newer(cat, m, o))
}

fn module_newer(cat: &Category, m: &ModuleSrc, o: SystemTime) -> bool {
    mtime(&m.file).is_none_or(|t| t > o)
        || dir_has_newer(&cat.dir.join(&m.name), o)
        || cat.shared.iter().any(|s| dir_has_newer(&cat.dir.join(s), o))
}

/// Has any input of `unit` changed since `t`? A failed unit's wasm is never
/// rewritten, so this, not the wasm, says when it is worth compiling again.
fn any_newer(src: &GameSrc, unit: &Unit, t: SystemTime) -> bool {
    let Unit::Module { category, name, .. } = unit else { return true };
    let Some(cat) = src.category(category) else { return true };
    if build::is_additive(category) {
        return dir_has_newer(&cat.dir, t);
    }
    cat.modules.iter().find(|m| &m.name == name).is_none_or(|m| module_newer(cat, m, t))
}

/// How the IDE names a unit: `"game/game2"`, `"process"`, `"asset/sprites"`.
fn key(unit: &Unit) -> String {
    match unit {
        Unit::Contract => "contract".into(),
        Unit::Module { category, .. } if build::is_additive(category) => category.clone(),
        Unit::Module { category, name, .. } => format!("{category}/{name}"),
    }
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

/// One unit that did not reload: the compile or the load refused it, and
/// the module that was running is still the one in its slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub unit: String,
    pub message: String,
}

/// What one poll did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    /// The contract moved: the caller reopens the game.
    Restart,
    Changed { ok: Vec<String>, failed: Vec<Failure> },
}

/// Owns what a poll needs between frames: where the game is, where its
/// outputs are, and which game and render the run chose.
pub struct Reloader {
    root: PathBuf,
    out: PathBuf,
    sel: Selection,
    profile: String,
    /// When each unit last failed, so the same broken input is compiled once.
    failed_at: HashMap<String, SystemTime>,
    /// When the last poll began: what a failure is dated with.
    polled_at: SystemTime,
    /// One per `build::compile` call.
    pub compiles: u32,
}

impl Reloader {
    pub fn new(root: PathBuf, out: PathBuf, sel: Selection, profile: String) -> Self {
        Reloader {
            root,
            out,
            sel,
            profile,
            failed_at: HashMap::new(),
            polled_at: SystemTime::UNIX_EPOCH,
            compiles: 0,
        }
    }

    /// Follow what is loaded now, so a swap from the Project panel is watched.
    pub fn select(&mut self, sel: Selection) {
        self.sel = sel;
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
mod tests;
