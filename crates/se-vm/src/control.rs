//! Writers for the host-owned control vars (keyboard, pad, clock), and the
//! one reader: a sound request a game wrote.
//!
//! Split out of `vm.rs` to keep that file under the project's 400-line
//! convention; these are `impl Vm` blocks like any other, just in their own
//! module because they share one shape (check the contract's declared
//! record, then touch bytes at the address it names).

use crate::clock;
use crate::input::{self, Keys, Pad};
use crate::sound;
use crate::vm::Vm;
use crate::world::Meta;
use anyhow::{bail, Context, Result};

/// Where a control var sits, once checked against the contract it was
/// declared against — or `None` if the contract declares no such control var
/// at all, which is a choice its contract made, not an error here.
///
/// Shared by all four control vars so they can only ever disagree on name,
/// expected byte length and field list, never on how a field is matched.
/// Field names are matched hyphen-insensitively (`f.name.replace('-', "_")
/// == name`) because WIT identifiers are kebab-case and the Rust side of
/// this engine spells the same field with an underscore — `input`/`pad`
/// relied on this before the fields were ever shared out of this helper;
/// `clock`/`sound` now get the same treatment rather than a second,
/// silently different copy.
fn check_control(
    meta: &Meta,
    name: &str,
    want_len: u32,
    fields: &[(&str, u32)],
) -> Result<Option<u32>> {
    let Some((at, len)) = meta.control_at(name) else {
        return Ok(None);
    };
    if len != want_len {
        bail!("`control.{name}` is {len} bytes; this engine expects {want_len}")
    }
    let r = meta.record(name).with_context(|| {
        format!("the contract names a `{name}` control var but declares no record")
    })?;
    for (fname, off) in fields {
        let f = r
            .fields
            .iter()
            .find(|f| f.name.replace('-', "_") == *fname)
            .with_context(|| format!("`control.{name}` has no `{fname}`"))?;
        if f.offset != *off {
            bail!(
                "`control.{name}.{fname}` is at {}; this engine expects {off}",
                f.offset
            )
        }
    }
    Ok(Some(at))
}

impl Vm {
    /// Hand the keyboard to the game.
    ///
    /// Checked against the contract the first time, field by field: a game
    /// that spells `input` differently would otherwise be fed three bitmasks
    /// at the wrong offsets and see keys nobody pressed.
    pub fn set_input(&mut self, keys: Keys) -> Result<()> {
        // A game that declares no `input` simply cannot be typed at. That is
        // a choice its contract made, not an error here.
        let Some(at) = check_control(&self.meta, "input", input::BYTES, &input::FIELDS)? else {
            return Ok(());
        };
        self.write(at, &keys.to_bytes())
    }

    /// Hand the gamepad to the game. Optional: a contract that declares no
    /// `pad` simply cannot be typed at, same rule as `set_input`.
    pub fn set_pad(&mut self, pad: Pad) -> Result<()> {
        let Some(at) = check_control(&self.meta, "pad", input::PAD_BYTES, &input::PAD_FIELDS)?
        else {
            return Ok(());
        };
        self.write(at, &pad.to_bytes())
    }

    /// Write `control.clock`. Time is the host's: `run_frame` calls this before
    /// every tick, so no game ever keeps its own. Checked like `set_input`.
    pub fn set_clock(&mut self, c: clock::Clock) -> Result<()> {
        let Some(at) = check_control(&self.meta, "clock", clock::BYTES, &clock::FIELDS)? else {
            return Ok(());
        };
        self.write(at, &c.to_bytes())
    }

    /// Edge-detect a sound request. `None` unless `sound.seq` changed since
    /// the last call — plain `!=`, which reads right across the one
    /// wraparound in four billion requests too. A contract with no `sound`
    /// control var never fires, same optional rule as `set_input`/`set_pad`.
    ///
    /// `load_save` deliberately never touches `self.last_sound_seq`: the
    /// comparison below already gives "equal to what this Vm last reported
    /// → silent, differs → fires once", restored arena or not.
    pub fn take_sound(&mut self) -> Result<Option<sound::SoundEvent>> {
        let Some(at) = check_control(&self.meta, "sound", sound::BYTES, &sound::FIELDS)? else {
            return Ok(None);
        };
        let b = self.read(at, sound::BYTES)?;
        let seq = u32::from_le_bytes(b[0..4].try_into().unwrap());
        let id = u32::from_le_bytes(b[4..8].try_into().unwrap());
        let gain = f32::from_le_bytes(b[8..12].try_into().unwrap());
        let changed = seq != self.last_sound_seq;
        self.last_sound_seq = seq;
        Ok(changed.then_some(sound::SoundEvent { id, gain }))
    }
}

#[cfg(test)]
mod tests {
    use super::check_control;
    use crate::world::Meta;
    use se_contract::layout::Ty;
    use se_contract::read::{ControlInfo, FieldInfo, RecordInfo};

    /// A `Meta` with exactly one control var, its record field spelled with a
    /// hyphen — the same shape `mouse-x`/`mouse-down` give `input`/`pad`.
    fn meta_with_hyphenated_field(name: &str, len: u32, field: &str, offset: u32) -> Meta {
        Meta {
            base: 0,
            frame: 0,
            commands: 0,
            pools_at: 0,
            columns_at: 0,
            blobs_at: 0,
            heap: 0,
            max_commands: 0,
            blob_slots: 0,
            components: vec![],
            all: vec![RecordInfo {
                name: name.to_string(),
                iface: 0,
                size: len,
                align: 4,
                fields: vec![FieldInfo {
                    name: field.to_string(),
                    ty: Ty::U32,
                    offset,
                }],
            }],
            pools: vec![],
            control: vec![ControlInfo {
                name: name.to_string(),
                at: 0x1000,
                len,
            }],
        }
    }

    // Before this helper existed, `set_clock`/`take_sound` matched a field by
    // `f.name == name` while `set_input`/`set_pad` matched by
    // `f.name.replace('-', "_") == name` — so a `clock`/`sound` record field
    // spelled with a hyphen, which WIT allows the same as any other
    // identifier, was invisible to `set_clock`/`take_sound` but not to
    // `set_input`/`set_pad`. `check_control` is now the only place either
    // kind of control var matches a field, so both get the same answer.
    #[test]
    fn clock_style_control_var_matches_a_hyphenated_field_name() {
        let m = meta_with_hyphenated_field("clock", 4, "frame-count", 0);
        let at = check_control(&m, "clock", 4, &[("frame_count", 0)]).unwrap();
        assert_eq!(at, Some(0x1000));
    }

    #[test]
    fn sound_style_control_var_matches_a_hyphenated_field_name() {
        let m = meta_with_hyphenated_field("sound", 4, "sound-id", 0);
        let at = check_control(&m, "sound", 4, &[("sound_id", 0)]).unwrap();
        assert_eq!(at, Some(0x1000));
    }

    #[test]
    fn an_undeclared_control_var_is_none_not_an_error() {
        let m = meta_with_hyphenated_field("clock", 4, "frame-count", 0);
        assert_eq!(
            check_control(&m, "pad", 4, &[("frame_count", 0)]).unwrap(),
            None
        );
    }
}
