//! Reading the layout rule off the filesystem.
//!
//! > `contract.wit` at the root is the contract. A `.rs` at depth 1 of a
//! > category is a module. Anything deeper is a source file inside one.
//!
//! That is the entire configuration format. There is no manifest, because a
//! manifest could disagree with the tree — and no `bundle/` directory any
//! more, because the contract is one file and it is not Rust.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Only `process` and `game` carry code. `asset` and `render` are data — an
/// asset is bytes and a graph is a declaration — but both still arrive as
/// `.wasm`, because a module with no functions is still a wasm module and one
/// loader is better than two.
pub const CATEGORIES: [&str; 4] = ["asset", "process", "render", "game"];

#[derive(Debug, Clone)]
pub struct ModuleSrc {
    /// Module name — the file stem.
    pub name: String,
    pub file: PathBuf,
}

#[derive(Debug)]
pub struct Category {
    pub name: String,
    pub dir: PathBuf,
    pub modules: Vec<ModuleSrc>,
    /// Folders with no sibling `.rs`: shared across the category.
    pub shared: Vec<String>,
}

#[derive(Debug)]
pub struct GameSrc {
    pub name: String,
    pub root: PathBuf,
    pub contract: PathBuf,
    pub categories: Vec<Category>,
}

fn entries(dir: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for e in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let p = e?.path();
        if p.file_name().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with('.')) {
            continue;
        }
        if p.is_dir() {
            dirs.push(p);
        } else if p.extension().is_some_and(|e| e == "rs") {
            files.push(p);
        }
    }
    files.sort();
    dirs.sort();
    Ok((files, dirs))
}

fn stem(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

impl GameSrc {
    pub fn open(root: &Path) -> Result<GameSrc> {
        if !root.is_dir() {
            bail!("{} is not a directory", root.display());
        }
        let name = root.file_name().and_then(|s| s.to_str()).unwrap_or("game").to_string();

        let contract = root.join("contract.wit");
        if !contract.is_file() {
            bail!(
                "`{name}` has no contract.wit. It is the one required file: every module \
                 compiles against what it declares, and the host reads it to know the \
                 shape of the world."
            );
        }

        let mut categories = Vec::new();
        for cat in CATEGORIES {
            let dir = root.join(cat);
            if !dir.is_dir() {
                continue;
            }
            let (files, dirs) = entries(&dir)?;
            let names: Vec<String> = files.iter().map(|f| stem(f)).collect();
            let modules = files
                .iter()
                .map(|f| ModuleSrc {
                    name: stem(f),
                    file: f.clone(),
                })
                .collect();
            // A folder named after a sibling `.rs` is private to that module;
            // one without is shared across the category.
            let shared = dirs
                .iter()
                .map(|d| stem(d))
                .filter(|d| !names.contains(d))
                .collect();
            categories.push(Category { name: cat.to_string(), dir, modules, shared });
        }
        Ok(GameSrc { name, root: root.to_path_buf(), contract, categories })
    }

    pub fn category(&self, name: &str) -> Option<&Category> {
        self.categories.iter().find(|c| c.name == name)
    }

    /// Asset names in id order: an asset's id is its index here, so a game
    /// names one by `assets::NAME` and the id only moves when the set does.
    pub fn asset_ids(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.category("asset").map_or_else(Vec::new, |c| c.modules.iter().map(|m| m.name.clone()).collect());
        v.sort();
        v
    }

}
