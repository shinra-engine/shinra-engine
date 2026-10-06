//! A broken edit is a value, compiled once, and retried after the next edit.

use super::apply_game::{changed, copy_game2, edit};
use super::{Outcome, Reloader, Selection};
use crate::deck::Deck;
use crate::live::{self, Live};
use crate::run::Options;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

fn half_w(l: &Live) -> f32 {
    let (at, _) = l.vm.meta.control_at("view").expect("view is a control var");
    f32::from_le_bytes(l.vm.read(at, 4).unwrap().try_into().unwrap())
}

/// Saved now, as an editor would: older than the poll that follows it.
fn bump(p: &Path) {
    let f = fs::File::options().write(true).open(p).unwrap();
    f.set_modified(SystemTime::now()).unwrap();
}

#[test]
fn a_broken_tick_fails_once_and_a_fix_reloads() {
    let root = copy_game2(&format!("game2-failure-{}", std::process::id()));
    let deck = Deck::open(&root).unwrap();
    let opts = Options { profile: "debug".into(), frames: None, game: None, render: None, watch: false };
    let size = (64, 36);
    eprintln!("standing game2 up");
    let mut live = live::open(&deck, &opts, size, &mut Vec::new()).unwrap();
    let out = Path::new("target").join("shinra").join(&deck.src().name);
    let mut rl = Reloader::new(root.clone(), out, Selection { game: None, render: None }, "debug".into());
    live.vm.frame(1).unwrap();
    assert_eq!(half_w(&live), 6.4);

    // a. A syntax error where the tick is: one failure, the old module stays.
    let game = root.join("game/game2.rs");
    let good = fs::read_to_string(&game).unwrap();
    edit(&game, "pub extern \"C\" fn tick", "pub extern \"C\" fn tick(((");
    bump(&game);
    eprintln!("polling the broken tick");
    let Outcome::Changed { ok, failed } = rl.poll(&mut live, size).unwrap() else { panic!("not Changed") };
    assert!(ok.is_empty(), "{ok:?}");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0].unit, "game/game2");
    assert!(failed[0].message.contains("error"), "{}", failed[0].message);
    live.vm.frame(2).unwrap();
    assert_eq!(half_w(&live), 6.4);
    for i in 0..3 {
        eprintln!("poll {i} with nothing new");
        assert_eq!(rl.poll(&mut live, size).unwrap(), Outcome::Nothing);
    }
    assert_eq!(rl.compiles, 1, "the same broken input is compiled once");
    live.vm.frame(3).unwrap();

    // b. The fix is newer than the failure, so it is tried and it loads.
    fs::write(&game, &good).unwrap();
    bump(&game);
    eprintln!("polling the fix");
    assert_eq!(rl.poll(&mut live, size).unwrap(), changed(&["game/game2"]));
    assert_eq!(rl.compiles, 2);
    live.vm.frame(4).unwrap();
    assert_eq!(half_w(&live), 6.4);

    let _ = fs::remove_dir_all(&root);
}
