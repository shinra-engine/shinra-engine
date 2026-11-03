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

/// Proves the whole chain end to end: a contract that declares `pad`, a game
/// module that reads `arena::PAD` and writes an observable control var from a
/// button bit, driven by a real [`ScriptedPad`] through the same
/// `set_pad`/`Vm::frame` sequence `shot::run` and `run::run` use. Lives here
/// rather than in `se-vm`'s or `se-cli`'s `tests/` because `ScriptedPad` is
/// private to this binary crate (no `se-cli` library target exists for an
/// integration test to import), so only code built into the crate itself can
/// drive a real `Vm` with the real scripted source.
#[cfg(test)]
mod vm_integration {
    use super::*;
    use se_vm::memory::{Slot, STACK_SIZE};
    use se_vm::Vm;
    use std::path::Path;
    use std::process::Command;

    /// `pad` is optional and `look` here is the whole point: one record for
    /// the thing under test, nothing a process/render module would need.
    /// `input` and `clock` come from `shinra:engine` regardless; omitted here
    /// since the game reads only `pad`. `World::place` refuses a contract
    /// with no pool at all, so `unused` exists only to satisfy that — the
    /// game never touches it.
    const WIT: &str = "package shinra:padfix@0.1.0;

interface components {
  record unused { v: f32 }
}

interface pools {
  use components.{unused};
  record unused-pool { u: unused }
}

interface control {
  record look { mark: u32 }
  record pad {
    down: u32,
    pressed: u32,
    released: u32,
    lx: f32,
    ly: f32,
    rx: f32,
    ry: f32,
    lt: f32,
    rt: f32,
  }
}
";

    /// Bit 0 is `south` ([`crate::pad::button_bit`]); hard-coded rather than
    /// pulled from `se_vm::input` because this source compiles to wasm32 on
    /// its own, outside this host crate's dependency graph.
    const GAME: &str = "#![no_std]
#[panic_handler]
fn se_panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }
include!(\"contract.rs\");
#[used]
#[link_section = \"se.contract-id\"]
pub static SE_CONTRACT_ID: [u8; 8] = CONTRACT_ID.to_le_bytes();

#[no_mangle]
pub extern \"C\" fn tick(_n: u32) -> u32 {
    unsafe {
        let pad = &*(arena::PAD as *const Pad);
        let look = &mut *(arena::LOOK as *mut Look);
        look.mark = pad.down & 1;
    }
    0
}
";

    fn has_wasm_target() -> bool {
        let Ok(o) = Command::new("rustc")
            .args([
                "--print",
                "target-libdir",
                "--target",
                "wasm32-unknown-unknown",
            ])
            .output()
        else {
            return false;
        };
        o.status.success() && Path::new(String::from_utf8_lossy(&o.stdout).trim()).is_dir()
    }

    /// Builds `game.wasm` the same way `build::compile` does, minus the `se`
    /// crate dependency this fixture's `tick` has no need of.
    fn build_game(dir: &Path) -> anyhow::Result<Vec<u8>> {
        let src = dir.join("game.rs");
        std::fs::write(&src, GAME)?;
        let wasm = dir.join("game.wasm");
        let out = Command::new("rustc")
            .args(["--edition", "2021", "--target", "wasm32-unknown-unknown"])
            .args(["--crate-type", "cdylib", "--crate-name", "game"])
            .arg("-C")
            .arg("link-arg=--import-memory")
            .arg("-C")
            .arg(format!(
                "link-arg=--global-base={}",
                Slot::Game.global_base()
            ))
            .arg("-C")
            .arg(format!("link-arg=-zstack-size={STACK_SIZE}"))
            .arg("-C")
            .arg("opt-level=1")
            .arg("-C")
            .arg("panic=abort")
            .arg("-o")
            .arg(&wasm)
            .arg(&src)
            .output()?;
        if !out.status.success() {
            anyhow::bail!(
                "building game.wasm failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(std::fs::read(wasm)?)
    }

    /// Runs the script through a fresh `Vm` and returns `look.mark` read back
    /// after every frame — the chain's one observable output.
    fn run_script(contract: &[u8], game: &[u8], script: &Path) -> anyhow::Result<Vec<u32>> {
        let mut vm = Vm::new(contract, 1)?;
        vm.load(Slot::Game, "game", game.to_vec())?;
        let mut ps = ScriptedPad::open(script)?;
        let (at, len) = vm
            .meta
            .control_at("look")
            .expect("look is a declared control var");
        let mut marks = Vec::new();
        for f in 0..5u32 {
            vm.set_pad(ps.poll(f))?;
            vm.frame(f)?;
            let b = vm.read(at, len)?;
            marks.push(u32::from_le_bytes(b[0..4].try_into().unwrap()));
        }
        Ok(marks)
    }

    #[test]
    fn scripted_pad_drives_a_real_game_through_set_pad_and_frame() -> anyhow::Result<()> {
        if !has_wasm_target() {
            eprintln!("skipped: wasm32-unknown-unknown is not installed");
            return Ok(());
        }
        let dir = std::env::temp_dir().join(format!("se-cli-pad-vm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        let wit = dir.join("contract.wit");
        std::fs::write(&wit, WIT)?;
        let g = se_contract::generate(&wit)?;
        std::fs::write(dir.join("contract.rs"), &g.rust)?;
        let game = build_game(&dir)?;

        // south held on frames 1-2, released by frame 3; frame 0 and the
        // repeated frame 4 are never pressed.
        let script = dir.join("pad.script");
        std::fs::write(&script, "-\nsouth\nsouth\n-\n-\n")?;

        let first = run_script(&g.module, &game, &script)?;
        let second = run_script(&g.module, &game, &script)?;

        assert_eq!(
            first,
            [0, 1, 1, 0, 0],
            "mark must reflect pad.down on exactly the pressed frames"
        );
        assert_eq!(
            first, second,
            "the same script must produce identical output every run"
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
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
