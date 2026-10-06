//! Compile a stale module and swap it in, between two frames.
//!
//! Only the units the plan named are compiled. `swap` loads the new bytes and
//! puts the previous module back if the load or the pipeline fails, so a bad
//! edit leaves the running game exactly as it was. `process` replaces every
//! stage at once; an asset is never instantiated, only its `se.asset` payload
//! moves into its blob slot. The world is not touched.

use super::{Reloader, Unit};
use crate::build::{self, Engine};
use crate::discover::GameSrc;
use crate::live::Live;
use crate::swap;
use anyhow::{bail, Context, Result};
use se_vm::memory::Slot;

impl Reloader {
    /// Called by the loop between frames, never from inside one. Returns the
    /// units swapped, as `"game/game2"`, `"process"` or `"asset/sprites"`.
    pub fn apply(&self, live: &mut Live, size: (u32, u32), units: &[Unit]) -> Result<Vec<String>> {
        // Refuse what this cannot do before compiling anything.
        for u in units {
            match u {
                Unit::Contract => bail!("the contract changed: that is a restart, not a reload"),
                Unit::Module { category, name, .. }
                    if !matches!(category.as_str(), "game" | "render" | "process" | "asset") =>
                {
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
            let Unit::Module { category, name, members, wasm } = u else { unreachable!("checked") };
            let cat = src.category(category).with_context(|| format!("no {category}/ in `{}`", src.name))?;
            let members: Vec<&str> = members.iter().map(String::as_str).collect();
            build::compile(name, &members, cat, &self.out, &gen, &contract_rs, &engine, &self.profile)?;
            match category.as_str() {
                "process" => {
                    // Every stage at once: `Vm::load` swaps the stage table
                    // only when the whole module loads.
                    live.vm.load(Slot::Process, name, std::fs::read(wasm)?)?;
                    swapped.push("process".to_string());
                }
                "asset" => {
                    // The slot is the module's position, as `live::open` put it.
                    let index = cat.modules.iter().position(|m| &m.name == name).context("asset vanished")?;
                    let bytes = std::fs::read(wasm)?;
                    let payload = se_contract::wasm::section(&bytes, "se.asset")
                        .with_context(|| format!("asset/{name} carries no se.asset section"))?;
                    live.vm.put_blob(index as u32, payload)?;
                    swapped.push(format!("asset/{name}"));
                }
                _ => {
                    swap::swap(live, &src, &self.out, category, name, size)?;
                    swapped.push(format!("{category}/{name}"));
                }
            }
        }
        Ok(swapped)
    }
}
