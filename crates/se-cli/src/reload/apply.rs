//! Compile a stale module and swap it in, between two frames.
//!
//! Only the units the plan named are compiled. `swap` loads the new bytes and
//! puts the previous module back if the load or the pipeline fails, so a bad
//! edit leaves the running game exactly as it was. `process` replaces every
//! stage at once; an asset is never instantiated, only its `se.asset` payload
//! moves into its blob slot. The world is not touched. A unit that fails is
//! reported as a value, and the units beside it still reload.

use super::{any_newer, dir_has_newer, key, Failure, Outcome, Plan, Reloader, Unit};
use crate::build::{self, Engine};
use crate::discover::GameSrc;
use crate::live::Live;
use crate::swap;
use anyhow::{bail, Context, Result};
use se_vm::memory::Slot;
use std::time::SystemTime;

impl Reloader {
    /// Called by the loop between frames, never from inside one.
    ///
    /// One poll: plan, then compile and swap each stale unit on its own. A
    /// unit that fails is a `Failure`, never an `Err`: the others still
    /// reload, and its module stays as it was. A failed unit is retried only
    /// once one of its inputs is newer than the failure.
    pub fn poll(&mut self, live: &mut Live, size: (u32, u32)) -> Result<Outcome> {
        self.polled_at = SystemTime::now();
        let units = match self.plan()? {
            Plan::Nothing => return Ok(Outcome::Nothing),
            Plan::Restart => {
                let held = self.failed_at.get(&key(&Unit::Contract));
                if held.is_some_and(|t| !dir_has_newer(&self.root, *t)) {
                    return Ok(Outcome::Nothing);
                }
                self.failed_at.clear();
                return Ok(Outcome::Restart);
            }
            Plan::Modules(units) => units,
        };
        let src = GameSrc::open(&self.root)?;
        let units: Vec<&Unit> = units
            .iter()
            .filter(|u| self.failed_at.get(&key(u)).is_none_or(|t| any_newer(&src, u, *t)))
            .collect();
        if units.is_empty() {
            return Ok(Outcome::Nothing);
        }
        let engine = Engine::find(&self.profile)?;
        let (mut ok, mut failed) = (Vec::new(), Vec::new());
        for u in units {
            let unit = key(u);
            match self.apply_one(live, size, &src, &engine, u) {
                Ok(_) => {
                    self.failed_at.remove(&unit);
                    ok.push(unit);
                }
                Err(e) => {
                    self.failed_at.insert(unit.clone(), self.polled_at);
                    failed.push(Failure { unit, message: format!("{e:#}") });
                }
            }
        }
        Ok(Outcome::Changed { ok, failed })
    }

    /// The caller could not reopen after a `Restart`: hold off until any
    /// source is newer than the poll that asked for it.
    pub fn restart_failed(&mut self) {
        self.failed_at.insert(key(&Unit::Contract), self.polled_at);
    }

    /// Compile one unit and swap it in. Returns its name, as `key` gives it.
    fn apply_one(
        &mut self,
        live: &mut Live,
        size: (u32, u32),
        src: &GameSrc,
        engine: &Engine,
        u: &Unit,
    ) -> Result<String> {
        // The same paths `build::build` writes, so a module compiles against
        // the contract the world was placed from.
        let gen = self.out.join(".gen");
        let contract_rs = self.out.join("contract.rs");
        let Unit::Module { category, name, members, wasm } = u else {
            bail!("the contract changed: that is a restart, not a reload")
        };
        if !matches!(category.as_str(), "game" | "render" | "process" | "asset") {
            bail!("{category}/{name} cannot be reloaded by a swap")
        }
        let cat = src.category(category).with_context(|| format!("no {category}/ in `{}`", src.name))?;
        let members: Vec<&str> = members.iter().map(String::as_str).collect();
        self.compiles += 1;
        build::compile(name, &members, cat, &self.out, &gen, &contract_rs, engine, &self.profile, &src.asset_ids())?;
        Ok(match category.as_str() {
            "process" => {
                // Every stage at once: `Vm::load` swaps the stage table
                // only when the whole module loads.
                live.vm.load(Slot::Process, name, std::fs::read(wasm)?)?;
                "process".to_string()
            }
            "asset" => {
                // The slot is the asset's id: its place among the sorted names.
                let index = src.asset_ids().iter().position(|n| n == name).context("asset vanished")?;
                let Live { vm, gpu, .. } = live;
                crate::live::load_asset(vm, gpu, index as u32, name, wasm)?;
                format!("asset/{name}")
            }
            _ => {
                swap::swap(live, src, &self.out, category, name, size)?;
                format!("{category}/{name}")
            }
        })
    }
}
