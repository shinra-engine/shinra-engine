//! Undoing a swap whose first frame traps.
//!
//! `load` refuses what it can see in the file. What it cannot see is a module
//! that is well formed and traps the first time it runs. So a swap is not
//! final at `load`: the module it replaced is kept, and the first frame
//! decides. A frame that returns confirms every pending swap; one that fails
//! puts the previous modules back.
//!
//! The previous module is re-instantiated from its bytes, not reinstated as
//! the old instance: instantiating the new one already wrote its data
//! segments over the old statics, at the same `--global-base`. A fresh
//! instance writes the old ones back.
//!
//! Only the oldest pending module per slot is kept. Two swaps before a frame
//! roll back to the one that last ran, which is the one known to work. The
//! first load of an empty slot keeps nothing: rolling back would empty it.
//!
//! Known limit: the world is not transactional. A trap after partial writes
//! leaves those writes in place. That is acceptable because state lives in
//! control vars and columns, which the next good tick rewrites.

use crate::memory::Slot;
use crate::module::Loaded;
use crate::stages::Stage;
use crate::Vm;
use anyhow::{Error, Result};

pub(crate) struct Staged {
    slot: Slot,
    prev: Loaded,
    prev_stages: Vec<Stage>,
}

impl Vm {
    /// Keep `prev`, the module `slot` held before this load, unless an older
    /// one is already kept for that slot.
    pub(crate) fn stage(&mut self, slot: Slot, prev: Option<Loaded>, prev_stages: Vec<Stage>) {
        let Some(prev) = prev else { return };
        if self.staged.iter().any(|s| s.slot == slot) {
            return;
        }
        self.staged.push(Staged { slot, prev, prev_stages });
    }

    /// A frame (or a recording) came back. `Ok` confirms the swaps in `slots`;
    /// `Err` rolls every pending one back and says so in the error.
    pub(crate) fn settle<T>(&mut self, r: Result<T>, slots: &[Slot]) -> Result<T> {
        match r {
            Ok(v) => {
                self.staged.retain(|s| !slots.contains(&s.slot));
                Ok(v)
            }
            Err(e) if self.staged.is_empty() => Err(e),
            Err(e) => Err(self.rollback(e)),
        }
    }

    fn rollback(&mut self, e: Error) -> Error {
        let mut slots = Vec::new();
        for s in std::mem::take(&mut self.staged) {
            let Staged { slot, prev, prev_stages } = s;
            let back = Loaded::prepare(&self.engine, slot, &prev.name, prev.bytes, self.contract_id())
                .and_then(|p| Loaded::commit(p, &mut self.store, self.memory));
            match back {
                Ok(m) => {
                    if slot == Slot::Process {
                        self.stages = prev_stages;
                    }
                    *self.slot_mut(slot) = Some(m);
                    slots.push(slot);
                }
                Err(re) => {
                    return e.context(format!(
                        "rolling back the {} slot also failed: {re:#}",
                        slot.name()
                    ))
                }
            }
        }
        let shown: Vec<&str> = slots.iter().map(|s| s.name()).collect();
        self.rolled_back = Some(slots);
        e.context(format!("rolled back {} to the previous module", shown.join(", ")))
    }

    /// The slots the last failed frame put back, once.
    pub fn take_rollback(&mut self) -> Option<Vec<Slot>> {
        self.rolled_back.take()
    }

    pub(crate) fn slot_mut(&mut self, slot: Slot) -> &mut Option<Loaded> {
        match slot {
            Slot::Game => &mut self.game,
            Slot::Process => &mut self.process,
            Slot::Render => &mut self.render,
        }
    }
}

