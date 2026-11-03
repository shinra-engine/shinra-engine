//! Headless gamepad input: a pluggable source polled once per frame.
//!
//! `shinra shot` has no terminal and no device, so there is no way to drive a
//! game's `pad` control var at all today. [`ScriptedPad`] fixes that for both
//! `shot` and `run`: a fixed, deterministic sequence of button states read
//! once from a file, polled the same way the keyboard is — before the tick,
//! at a frame index — and folded into `pressed`/`released` edges by comparing
//! consecutive polls, never by a timer (that is `se-tui`'s keyboard-specific
//! HELD_FOR hack, and it has no place here).

use anyhow::{Context, Result};
use se_vm::input::Pad;
use std::path::Path;

/// Where a frame's gamepad state comes from. One implementation today: a
/// scripted source read up front. A real device would be another.
pub trait PadSource {
    fn poll(&mut self, frame: u32) -> Pad;
}

/// One scripted frame: which buttons are held. Sticks and triggers are not
/// expressible in the file format (nothing here needs them yet) and are
/// always zero.
#[derive(Clone, Copy, Default)]
struct Frame {
    down: u32,
}

/// Replays a fixed sequence of [`Frame`]s, one per call to `poll`, deriving
/// `pressed`/`released` from consecutive `down` masks — the same rule
/// [`Pad::from_held`] (and the keyboard's `Keys::from_held`) uses. Once the
/// script runs out, the last frame repeats, so a short script still drives
/// an arbitrarily long run deterministically.
pub struct ScriptedPad {
    frames: Vec<Frame>,
    was: u32,
}

impl ScriptedPad {
    fn new(frames: Vec<Frame>) -> Self {
        ScriptedPad { frames, was: 0 }
    }

    /// Reads the line format [`parse`] describes.
    pub fn open(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(ScriptedPad::new(parse(&text)?))
    }
}

impl PadSource for ScriptedPad {
    fn poll(&mut self, frame: u32) -> Pad {
        let down = self
            .frames
            .get(frame as usize)
            .or(self.frames.last())
            .map(|f| f.down)
            .unwrap_or(0);
        let pad = Pad::from_held(down, self.was, [0.0; 4], [0.0; 2]);
        self.was = down;
        pad
    }
}

/// One line per frame: a comma-separated list of button names (see
/// [`button_bit`]), or `-` for nothing held. Blank lines and lines starting
/// with `#` are skipped, so a script can carry a comment.
///
/// ```text
/// -
/// south
/// south
/// -
/// ```
fn parse(text: &str) -> Result<Vec<Frame>> {
    let mut frames = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut down = 0u32;
        if line != "-" {
            for name in line.split(',') {
                let name = name.trim();
                down |= button_bit(name).with_context(|| format!("unknown pad button `{name}`"))?;
            }
        }
        frames.push(Frame { down });
    }
    Ok(frames)
}

fn button_bit(name: &str) -> Option<u32> {
    use se_vm::input::*;
    Some(match name {
        "south" => PAD_SOUTH,
        "east" => PAD_EAST,
        "west" => PAD_WEST,
        "north" => PAD_NORTH,
        "lb" => PAD_LB,
        "rb" => PAD_RB,
        "back" => PAD_BACK,
        "start" => PAD_START,
        "lstick" => PAD_LSTICK,
        "rstick" => PAD_RSTICK,
        "dpad-up" => PAD_DPAD_UP,
        "dpad-down" => PAD_DPAD_DOWN,
        "dpad-left" => PAD_DPAD_LEFT,
        "dpad-right" => PAD_DPAD_RIGHT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use se_vm::input::PAD_SOUTH;

    #[test]
    fn held_across_two_frames_is_an_edge_not_a_level() {
        let mut pad = ScriptedPad::new(parse("south\nsouth\n-\n").unwrap());
        let f0 = pad.poll(0);
        assert_eq!(f0.pressed, PAD_SOUTH);
        assert_eq!(f0.released, 0);
        let f1 = pad.poll(1);
        assert_eq!(f1.pressed, 0);
        assert_eq!(f1.down, PAD_SOUTH);
        let f2 = pad.poll(2);
        assert_eq!(f2.released, PAD_SOUTH);
        assert_eq!(f2.down, 0);
    }

    #[test]
    fn repeats_the_last_frame_once_the_script_runs_out() {
        let mut pad = ScriptedPad::new(parse("south\n").unwrap());
        pad.poll(0);
        assert_eq!(pad.poll(5).down, PAD_SOUTH);
        assert_eq!(pad.poll(5).pressed, 0);
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let frames = parse("# a comment\n\nsouth\n").unwrap();
        assert_eq!(frames.len(), 1);
    }

    #[test]
    fn unknown_button_is_an_error() {
        assert!(parse("not-a-button\n").is_err());
    }
}
