//! A sound request's shape: what `control.sound` carries.
//!
//! Mirrors `clock.rs`/`input.rs`: the record's shape lives here; the host's
//! read of it — with the same contract-offset defence every control var
//! gets — lives in `control.rs`'s `take_sound`, alongside `set_input`,
//! `set_pad` and `set_clock`.

/// Size of `control.sound`.
pub const BYTES: u32 = 12;

/// `(field, offset)` in `shinra:engine`'s `sound`; `take_sound` checks the
/// contract's record against this before trusting the offsets below.
pub const FIELDS: [(&str, u32); 3] = [("seq", 0), ("id", 4), ("gain", 8)];

/// What a game asked for, once `take_sound` has stripped the repeats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundEvent {
    pub id: u32,
    pub gain: f32,
}
