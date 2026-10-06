//! A touched asset: its payload moves into the same blob slot and nothing is
//! instantiated. A payload that fits reuses the old span, so repeated reloads
//! do not grow the blob region.

use super::apply_game::{changed, copy_game2, sprites};
use super::{Plan, Reloader, Selection};
use crate::deck::Deck;
use crate::live;
use crate::run::Options;
use std::fs;
use std::path::Path;

fn touch(p: &Path) {
    let f = fs::File::options().write(true).open(p).unwrap();
    f.set_modified(std::time::SystemTime::now()).unwrap();
}

#[test]
fn apply_asset_puts_the_payload_and_does_not_leak() {
    let root = copy_game2(&format!("game2-asset-{}", std::process::id()));
    let deck = Deck::open(&root).unwrap();
    let opts = Options { profile: "debug".into(), frames: None, game: None, render: None, watch: false };
    let size = (64, 36);
    let mut live = live::open(&deck, &opts, size, &mut Vec::new()).unwrap();
    let out = Path::new("target").join("shinra").join(&deck.src().name);
    let mut rl = Reloader::new(root.clone(), out, Selection { game: None, render: None }, "debug".into());
    live.vm.frame(1).unwrap();
    let world = sprites(&live);
    let file = root.join("asset/sprites.rs");

    touch(&file);
    assert!(matches!(rl.plan().unwrap(), Plan::Modules(_)), "a touched asset is Modules");
    assert_eq!(rl.poll(&mut live, size).unwrap(), changed(&["asset/sprites"]));
    let top = live.vm.world.blob_top;
    assert_eq!(sprites(&live), world);

    touch(&file);
    assert!(matches!(rl.plan().unwrap(), Plan::Modules(_)), "touched again");
    assert_eq!(rl.poll(&mut live, size).unwrap(), changed(&["asset/sprites"]));
    assert_eq!(live.vm.world.blob_top, top, "a same-size payload reuses its span");
    assert_eq!(sprites(&live), world);
    live.vm.frame(2).unwrap();

    let _ = fs::remove_dir_all(&root);
}
