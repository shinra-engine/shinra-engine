//! Time, as a control var the host writes.
//!
//! A game does not keep the clock. If it did, every game would pick its own
//! step and the world would depend on how fast the machine happened to run;
//! here the host writes `control.clock` before every tick, from the frame
//! number alone: `t = frame * STEP`, `dt = STEP`. The world is then a function
//! of the arena and the input sequence, so two runs fed the same keys are
//! byte-identical — which is what makes a save, a replay and a screenshot
//! mean something.

/// One simulation step, in seconds.
pub const STEP: f32 = 1.0 / 60.0;

/// Size of `control.clock`.
pub const BYTES: u32 = 16;

/// `(field, offset)` in `shinra:engine`'s `clock`; the host checks the
/// contract's record against this before writing.
pub const FIELDS: [(&str, u32); 4] = [("t", 0), ("dt", 4), ("frame", 8), ("swipe", 12)];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    pub t: f32,
    pub dt: f32,
    pub frame: u32,
    pub swipe: i32,
}

impl Clock {
    /// The clock a game sees while frame `frame` ticks.
    pub fn at(frame: u32) -> Clock {
        Clock { t: frame as f32 * STEP, dt: STEP, frame, swipe: 0 }
    }

    pub fn to_bytes(self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&self.t.to_le_bytes());
        b[4..8].copy_from_slice(&self.dt.to_le_bytes());
        b[8..12].copy_from_slice(&self.frame.to_le_bytes());
        b[12..16].copy_from_slice(&self.swipe.to_le_bytes());
        b
    }
}
