//! An edit to one of game2's stages: the whole process module is rebuilt and
//! every stage replaced, under a running world.

use super::apply_game::{changed, copy_game2, edit, sprites};
use super::{Plan, Reloader, Selection};
use crate::deck::Deck;
use crate::live;
use crate::run::Options;
use std::fs;
use std::path::Path;

#[test]
fn apply_process_replaces_every_stage_and_keeps_the_world() {
    let root = copy_game2(&format!("game2-process-{}", std::process::id()));
    let deck = Deck::open(&root).unwrap();
    let opts = Options { profile: "debug".into(), frames: None, game: None, render: None, watch: false };
    let size = (64, 36);
    let mut live = live::open(&deck, &opts, size, &mut Vec::new()).unwrap();
    let out = Path::new("target").join("shinra").join(&deck.src().name);
    let mut rl = Reloader::new(root.clone(), out, Selection { game: None, render: None }, "debug".into());
    live.vm.frame(1).unwrap();
    let names = |l: &live::Live| l.vm.stages().iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    let before = names(&live);
    assert!(!before.is_empty());

    edit(&root.join("process/gravity.rs"), "const G: f32 = -26.0;", "const G: f32 = -13.0;");
    assert!(matches!(rl.plan().unwrap(), Plan::Modules(_)), "an edited stage is Modules");
    let world = sprites(&live);
    assert_eq!(rl.poll(&mut live, size).unwrap(), changed(&["process"]));
    assert_eq!(names(&live), before);
    assert_eq!(sprites(&live), world, "a reload moved no row");
    assert_eq!(rl.plan().unwrap(), Plan::Nothing);
    live.vm.frame(2).unwrap();

    let _ = fs::remove_dir_all(&root);
}
