//! A deck: the games you can swipe between.
//!
//! design.md calls the feel gametok — *像是短影音左滑一下就換了一款遊戲*. On a
//! keyboard that is `n`, and the unit the run loop works in is the deck rather
//! than one game, because switching is the point rather than a feature.
//!
//! A directory is a deck if its children are games; a directory with
//! `contract.wit` in it is one game, and a deck of one.

use crate::discover::GameSrc;
use anyhow::{bail, Context, Result};
use std::path::Path;

pub struct Deck {
    pub games: Vec<GameSrc>,
    pub current: usize,
}

impl Deck {
    pub fn open(root: &Path) -> Result<Deck> {
        if root.join("contract.wit").is_file() {
            return Ok(Deck { games: vec![GameSrc::open(root)?], current: 0 });
        }
        let mut dirs: Vec<_> = std::fs::read_dir(root)
            .with_context(|| format!("read {}", root.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir() && p.join("contract.wit").is_file())
            .collect();
        dirs.sort();
        if dirs.is_empty() {
            bail!(
                "{} is neither a game nor a deck: it has no contract.wit, and no child \
                 directory has one either",
                root.display()
            )
        }
        let games = dirs.iter().map(|d| GameSrc::open(d)).collect::<Result<Vec<_>>>()?;
        Ok(Deck { games, current: 0 })
    }

    pub fn names(&self) -> Vec<String> {
        self.games.iter().map(|g| g.name.clone()).collect()
    }

    pub fn src(&self) -> &GameSrc {
        &self.games[self.current]
    }

    /// Move by `delta`, wrapping. False when there is nowhere to go, so the
    /// caller can leave the current game standing rather than rebuild it.
    pub fn step(&mut self, delta: i32) -> bool {
        if self.games.len() < 2 || delta == 0 {
            return false;
        }
        let n = self.games.len() as i32;
        self.current = (((self.current as i32 + delta) % n + n) % n) as usize;
        true
    }
}
