//! A refused module and a trapping one both leave the old one ticking.
//!
//! The two are told apart: a refusal is a `Failure` from `poll`, a trap is a
//! reload that loads, then a frame that errs and is rolled back.

use super::apply_game::{changed, copy_game2, edit};
use super::failure::{bump, half_w};
use super::{Outcome, Reloader, Selection};
use crate::deck::Deck;
use crate::live::{self, Live};
use crate::run::Options;
use se_vm::memory::Slot;
use std::fs;
use std::path::Path;

/// `clock.frame`: after `t` and `dt`.
fn clock_frame(l: &Live) -> u32 {
    let (at, _) = l.vm.meta.control_at("clock").expect("clock is a control var");
    u32::from_le_bytes(l.vm.read(at + 8, 4).unwrap().try_into().unwrap())
}

#[test]
fn a_refused_and_a_trapping_tick_keep_the_old_one_running() {
    let root = copy_game2(&format!("game2-refusal-{}", std::process::id()));
    let deck = Deck::open(&root).unwrap();
    let opts = Options { profile: "debug".into(), frames: None, game: None, render: None, watch: false, pad: None };
    let size = (64, 36);
    eprintln!("standing game2 up");
    let mut live = live::open(&deck, &opts, size, &mut Vec::new()).unwrap();
    let out = Path::new("target").join("shinra").join(&deck.src().name);
    let mut rl = Reloader::new(root.clone(), out, Selection { game: None, render: None }, "debug".into());
    live.vm.frame(1).unwrap();
    assert_eq!(clock_frame(&live), 1);

    // c. `tock` compiles, but the game slot calls `tick`: `load` refuses it.
    let game = root.join("game/game2.rs");
    let good = fs::read_to_string(&game).unwrap();
    edit(&game, "pub extern \"C\" fn tick(", "pub extern \"C\" fn tock(");
    bump(&game);
    eprintln!("polling the renamed tick");
    let before = rl.compiles;
    let Outcome::Changed { ok, failed } = rl.poll(&mut live, size).unwrap() else { panic!("not Changed") };
    assert!(ok.is_empty(), "{ok:?}");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].unit, "game/game2");
    assert!(failed[0].message.contains("tick"), "{}", failed[0].message);
    assert_eq!(rl.compiles, before + 1);
    live.vm.frame(2).unwrap();
    assert_eq!(clock_frame(&live), 2, "the old tick still runs");
    eprintln!("polling again with nothing new");
    assert_eq!(rl.poll(&mut live, size).unwrap(), Outcome::Nothing);
    assert_eq!(rl.compiles, before + 1, "a refused input is compiled once");

    // e. The fix is newer than the refusal, so it reloads.
    fs::write(&game, &good).unwrap();
    bump(&game);
    eprintln!("polling the fix");
    assert_eq!(rl.poll(&mut live, size).unwrap(), changed(&["game/game2"]));
    live.vm.frame(3).unwrap();
    assert_eq!(clock_frame(&live), 3);

    // d. A tick that traps is well formed: it loads, and its first frame
    // fails and puts the previous module back.
    edit(&game, "fn tick(frame: u32) -> u32 {", "fn tick(frame: u32) -> u32 {\n    core::arch::wasm32::unreachable();");
    bump(&game);
    eprintln!("polling the trapping tick");
    assert_eq!(rl.poll(&mut live, size).unwrap(), changed(&["game/game2"]));
    assert!(live.vm.frame(4).is_err(), "the trapping tick runs and traps");
    assert_eq!(live.vm.take_rollback(), Some(vec![Slot::Game]));
    eprintln!("the frame after the rollback");
    live.vm.frame(5).unwrap();
    assert_eq!(half_w(&live), 6.4);
    assert_eq!(clock_frame(&live), 5);

    let _ = fs::remove_dir_all(&root);
}
