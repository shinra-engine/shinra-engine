//! Writers for the host-owned control vars: keyboard, pad, clock.
//!
//! Split out of `vm.rs` to keep that file under the project's 400-line
//! convention; these are `impl Vm` blocks like any other, just in their own
//! module because the three share one shape (check the contract's declared
//! record, then write bytes at the address it names).

use crate::clock;
use crate::input::{self, Keys, Pad};
use crate::vm::Vm;
use anyhow::{bail, Context, Result};

impl Vm {
    /// Hand the keyboard to the game.
    ///
    /// Checked against the contract the first time, field by field: a game
    /// that spells `input` differently would otherwise be fed three bitmasks
    /// at the wrong offsets and see keys nobody pressed.
    pub fn set_input(&mut self, keys: Keys) -> Result<()> {
        let Some((at, len)) = self.meta.control_at("input") else {
            // A game that declares no `input` simply cannot be typed at. That
            // is a choice its contract made, not an error here.
            return Ok(());
        };
        if len != input::BYTES {
            bail!(
                "`control.input` is {len} bytes; this engine writes {}",
                input::BYTES
            )
        }
        let r = self
            .meta
            .record("input")
            .context("the contract names an `input` control var but declares no record")?;
        for (name, off) in input::FIELDS {
            let f = r
                .fields
                .iter()
                .find(|f| f.name.replace('-', "_") == name)
                .with_context(|| format!("`control.input` has no `{name}`"))?;
            if f.offset != off {
                bail!(
                    "`control.input.{name}` is at {}; this engine writes {off}",
                    f.offset
                )
            }
        }
        self.write(at, &keys.to_bytes())
    }

    /// Hand the gamepad to the game. Optional: a contract that declares no
    /// `pad` simply cannot be typed at, same rule as `set_input`.
    pub fn set_pad(&mut self, pad: Pad) -> Result<()> {
        let Some((at, len)) = self.meta.control_at("pad") else {
            return Ok(());
        };
        if len != input::PAD_BYTES {
            bail!(
                "`control.pad` is {len} bytes; this engine writes {}",
                input::PAD_BYTES
            )
        }
        let r = self
            .meta
            .record("pad")
            .context("the contract names a `pad` control var but declares no record")?;
        for (name, off) in input::PAD_FIELDS {
            let f = r
                .fields
                .iter()
                .find(|f| f.name.replace('-', "_") == name)
                .with_context(|| format!("`control.pad` has no `{name}`"))?;
            if f.offset != off {
                bail!(
                    "`control.pad.{name}` is at {}; this engine writes {off}",
                    f.offset
                )
            }
        }
        self.write(at, &pad.to_bytes())
    }

    /// Write `control.clock`. Time is the host's: `run_frame` calls this before
    /// every tick, so no game ever keeps its own. Checked like `set_input`.
    pub fn set_clock(&mut self, c: clock::Clock) -> Result<()> {
        let Some((at, len)) = self.meta.control_at("clock") else {
            return Ok(());
        };
        if len != clock::BYTES {
            bail!(
                "`control.clock` is {len} bytes; this engine writes {}",
                clock::BYTES
            )
        }
        let r = self
            .meta
            .record("clock")
            .context("the contract names a `clock` control var but declares no record")?;
        for (name, off) in clock::FIELDS {
            let f = r
                .fields
                .iter()
                .find(|f| f.name == name)
                .with_context(|| format!("`control.clock` has no `{name}`"))?;
            if f.offset != off {
                bail!(
                    "`control.clock.{name}` is at {}; this engine writes {off}",
                    f.offset
                )
            }
        }
        self.write(at, &c.to_bytes())
    }
}
