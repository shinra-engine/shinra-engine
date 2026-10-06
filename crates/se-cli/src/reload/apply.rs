//! Compile a stale game or render module and swap it in, between two frames.
//!
//! Only the units the plan named are compiled. `swap` loads the new bytes and
//! puts the previous module back if the load or the pipeline fails, so a bad
//! edit leaves the running game exactly as it was. The world is not touched.

use super::{Reloader, Unit};
use crate::build::{self, Engine};
use crate::discover::GameSrc;
use crate::live::Live;
use crate::swap;
use anyhow::{bail, Context, Result};

impl Reloader {
    /// Called by the loop between frames, never from inside one. Returns the
    /// units swapped, as `"game/game2"`.
    pub fn apply(&self, live: &mut Live, size: (u32, u32), units: &[Unit]) -> Result<Vec<String>> {
        // Refuse what this cannot do before compiling anything.
        for u in units {
            match u {
                Unit::Contract => bail!("the contract changed: that is a restart, not a reload"),
                Unit::Module { category, name, .. } if category != "game" && category != "render" => {
                    bail!("{category}/{name} cannot be reloaded by a swap")
                }
                Unit::Module { .. } => {}
            }
        }
        let src = GameSrc::open(&self.root)?;
        let engine = Engine::find(&self.profile)?;
        // The same paths `build::build` writes, so a module compiles against
        // the contract the world was placed from.
        let gen = self.out.join(".gen");
        let contract_rs = self.out.join("contract.rs");
        let mut swapped = Vec::new();
        for u in units {
            let Unit::Module { category, name, members, .. } = u else { unreachable!("checked") };
            let cat = src.category(category).with_context(|| format!("no {category}/ in `{}`", src.name))?;
            let members: Vec<&str> = members.iter().map(String::as_str).collect();
            build::compile(name, &members, cat, &self.out, &gen, &contract_rs, &engine, &self.profile)?;
            swap::swap(live, &src, &self.out, category, name, size)?;
            swapped.push(format!("{category}/{name}"));
        }
        Ok(swapped)
    }
}
