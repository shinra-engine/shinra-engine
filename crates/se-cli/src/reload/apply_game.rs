//! An edit to game2's tick, compiled and swapped under a running world.

use super::{Plan, Reloader, Selection};
use crate::deck::Deck;
use crate::live::{self, Live};
use crate::run::Options;
use std::fs;
use std::path::{Path, PathBuf};

/// game2 copied to a temp dir, so editing it never touches the examples.
pub(super) fn copy_game2(tag: &str) -> PathBuf {
    let root = std::env::var("SHINRA_EXAMPLES")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../shinra-examples").into());
    let from = Path::new(&root).join("game2");
    let to = std::env::temp_dir().join(format!("se-apply-{}", std::process::id())).join(tag);
    let _ = fs::remove_dir_all(&to);
    let mut stack = vec![(from, to.clone())];
    while let Some((f, t)) = stack.pop() {
        fs::create_dir_all(&t).unwrap();
        for e in fs::read_dir(&f).unwrap().flatten() {
            let p = e.path();
            let n = e.file_name();
            if p.is_dir() {
                if n != "target" {
                    stack.push((p, t.join(n)));
                }
            } else {
                fs::copy(&p, t.join(n)).unwrap();
            }
        }
    }
    to
}

fn half_w(l: &Live) -> f32 {
    let (at, _) = l.vm.meta.control_at("view").expect("view is a control var");
    f32::from_le_bytes(l.vm.read(at, 4).unwrap().try_into().unwrap())
}

pub(super) fn sprites(l: &Live) -> (u32, Vec<u8>) {
    let pool = l.vm.meta.pool_index("sprites").expect("sprites pool") as usize;
    let col = l.vm.meta.column_index("sprite").expect("sprite column") as usize;
    let row0 = l.vm.read(l.vm.world.base[col], l.vm.world.stride[col]).unwrap();
    (l.vm.world.pool_len[pool], row0)
}

pub(super) fn edit(file: &Path, from: &str, to: &str) {
    let s = fs::read_to_string(file).unwrap();
    assert!(s.contains(from), "{from} not in {}", file.display());
    fs::write(file, s.replacen(from, to, 1)).unwrap();
}

#[test]
fn apply_game_swaps_the_tick_and_keeps_the_world() {
    let root = copy_game2(&format!("game2-apply-{}", std::process::id()));
    let deck = Deck::open(&root).unwrap();
    let opts = Options { profile: "debug".into(), frames: None, game: None, render: None, watch: false };
    let size = (64, 36);
    let mut live = live::open(&deck, &opts, size, &mut Vec::new()).unwrap();
    let out = Path::new("target").join("shinra").join(&deck.src().name);
    let rl = Reloader::new(root.clone(), out, Selection { game: None, render: None }, "debug".into());

    live.vm.frame(0).unwrap();
    live.vm.frame(1).unwrap();
    assert_eq!(half_w(&live), 6.4);
    assert_eq!(rl.plan().unwrap(), Plan::Nothing);

    let game = root.join("game/game2.rs");
    edit(&game, "view.half_w = 6.4;", "view.half_w = 7.5;");
    let Plan::Modules(units) = rl.plan().unwrap() else { panic!("an edited game is Modules") };
    let world = sprites(&live);
    let t = std::time::Instant::now();
    assert_eq!(rl.apply(&mut live, size, &units).unwrap(), ["game/game2"]);
    eprintln!("apply took {:?}", t.elapsed());
    assert_eq!(sprites(&live), world, "a swap moved no row");
    live.vm.frame(2).unwrap();
    assert_eq!(half_w(&live), 7.5);
    assert_eq!(sprites(&live), world, "the tick only moved the view");

    // A syntax error: the compile fails and the module that was running stays.
    edit(&game, "view.half_w = 7.5;", "view.half_w = = 7.5;");
    let Plan::Modules(units) = rl.plan().unwrap() else { panic!("still Modules") };
    let err = rl.apply(&mut live, size, &units).unwrap_err();
    eprintln!("refused: {}", format!("{err:#}").lines().next().unwrap_or(""));
    live.vm.frame(3).unwrap();
    assert_eq!(half_w(&live), 7.5);

    let _ = fs::remove_dir_all(&root);
}
