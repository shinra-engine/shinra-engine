//! story-66: a game writes the `sound` control var, `shinra shot` runs it
//! headless, and the headless sink's log line proves the whole channel —
//! `game.wasm` write -> `Vm::take_sound()` edge detection -> `se-cli`
//! `audio.rs` blob resolution -> `HeadlessSink::play` -> stderr log line.
//!
//! Not a proof of real audio playback: this host has no audio device
//! (verified in story-66's doc: `/dev/snd` carries only `timer`).
//!
//! The contract declares no `sound` record itself — `control.sound` is part
//! of `shinra:engine`'s own `control` interface (task-265), injected into
//! every contract the same way `clock` and `input` are, so a bare contract
//! already has `arena::SOUND` and a `Sound` struct.

use std::path::{Path, PathBuf};
use std::process::Command;

// A contract needs at least one pool (a column with no pool has no length).
// Nothing in this fixture draws `dummy` — it exists only to satisfy that
// rule; the game never asks for a nonzero roster.
const CONTRACT: &str = r#"package shinra:soundtest@0.1.0;

interface components {
  record dummy { v: f32 }
}

interface pools {
  use components.{dummy};

  record dummies { d: dummy }
}
"#;

/// Sets `sound.id/gain` and bumps `sound.seq` once, on frame 2 only. Every
/// other frame leaves the control var untouched, which is what proves
/// `Vm::take_sound()`'s edge detection: the fields stay set to the same
/// values after frame 2, but `seq` does not move again, so the sink must
/// not log a second time.
const GAME: &str = r#"use crate::*;

#[no_mangle]
pub extern "C" fn tick(frame: u32) -> u32 {
    unsafe {
        if frame == 2 {
            let s = &mut *(arena::SOUND as *mut Sound);
            s.id = assets::SOUND;
            s.gain = 0.5;
            s.seq = s.seq.wrapping_add(1);
        }
    }
    0
}
"#;

/// No stages; the `process/` directory only needs to exist so `shinra shot`
/// has something to fill the Process slot with (see `build::is_additive`).
const PROCESS: &str = "use crate::*;\n";

const SHADER: &str = "// no draws issued by this fixture\n";

/// `BEGIN_PASS` + `END_PASS` only: a clear with no draw, so no mesh/texture
/// asset is needed to get `shinra shot` past its \"recorded no commands\"
/// check.
const RENDER: &str = r#"use crate::*;
use crate::render::*;

#[used]
#[link_section = "se.wgsl"]
static WGSL: [u8; include_bytes!("flat/shader.wgsl").len()] = *include_bytes!("flat/shader.wgsl");

#[no_mangle]
pub extern "C" fn record() -> u32 {
    unsafe {
        let cmds = arena::COMMANDS as *mut Command;
        *cmds.add(0) = Command { kind: BEGIN_PASS, clear_r: 0.0, clear_g: 0.0, clear_b: 0.0, ..BLANK };
        *cmds.add(1) = Command { kind: END_PASS, ..BLANK };
        let f = &mut *(arena::FRAME as *mut Frame);
        f.commands = arena::COMMANDS;
        f.count = 2;
        f.present = 0;
        f.epoch = f.epoch.wrapping_add(1);
        2
    }
}
"#;

const ASSET: &str = "se::asset!(\"sound/beep.bin\");\n";
/// Content doesn't matter — `se_gpu::resources::decode` treats any kind but
/// `obj`/`png` as an opaque blob, so these bytes just pass through to
/// `vm.put_blob` and back out `vm.blob()` for `audio::dispatch` to log the
/// length of.
const BEEP: &[u8] = b"beep";

fn has_wasm_target() -> bool {
    let Ok(o) = Command::new("rustc")
        .args(["--print", "target-libdir", "--target", "wasm32-unknown-unknown"])
        .output()
    else {
        return false;
    };
    o.status.success() && Path::new(String::from_utf8_lossy(&o.stdout).trim()).is_dir()
}

fn put(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

#[test]
fn shinra_shot_logs_one_sound_event_and_never_repeats_it() {
    if !has_wasm_target() {
        eprintln!("skipped: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir: PathBuf = std::env::temp_dir().join(format!("shinra-sound-shot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let game = dir.join("game");
    put(&game, "contract.wit", CONTRACT.as_bytes());
    put(&game, "game/sound.rs", GAME.as_bytes());
    put(&game, "process/noop.rs", PROCESS.as_bytes());
    put(&game, "render/flat.rs", RENDER.as_bytes());
    put(&game, "render/flat/shader.wgsl", SHADER.as_bytes());
    put(&game, "asset/sound.rs", ASSET.as_bytes());
    put(&game, "asset/sound/beep.bin", BEEP);

    let out = dir.join("out");
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let run = Command::new(env!("CARGO_BIN_EXE_shinra"))
        .arg("shot")
        .arg(&game)
        .arg("-o")
        .arg(&out)
        .arg("-n")
        .arg("5")
        .arg("--size")
        .arg("8x8")
        .env("SHINRA_ENGINE", &engine)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "shinra shot failed:\n{stderr}");

    // `HeadlessSink::play` logs exactly `"sound: N bytes, gain G"`, with no
    // other prefix; `starts_with` (after trimming shot.rs's `"  "` indent)
    // keeps this from also matching the unrelated `"asset sound: ..."` load
    // line, which also contains the substring `"sound:"`.
    let sound_lines: Vec<&str> =
        stderr.lines().filter(|l| l.trim_start().starts_with("sound:")).collect();
    assert_eq!(
        sound_lines.len(),
        1,
        "expected exactly one sound event across 5 frames (seq bumped once, at frame 2); got:\n{stderr}"
    );
    assert!(
        sound_lines[0].contains(&format!("{} bytes", BEEP.len())),
        "sound log should resolve the asset blob's {} bytes: {}",
        BEEP.len(),
        sound_lines[0]
    );
    assert!(
        sound_lines[0].contains("0.500"),
        "sound log should carry the gain the game wrote: {}",
        sound_lines[0]
    );

    let _ = std::fs::remove_dir_all(&dir);
}
