//! `shinra` — build a game, and run it.
//!
//! ```text
//! shinra build <game>              compile the contract and every module to .wasm
//! shinra run   <game|deck>         build, then run it in the IDE
//! shinra shot  <game> [-n N]       render N frames headless, write a PPM
//! ```
//!
//! A directory whose children are games is a *deck*: `n` moves through it,
//! which is what gametok means by a swipe.
//!
//! A game is a directory with `contract.wit` at its root and `asset/`,
//! `process/`, `render/`, `game/` beside it. There is no project file to keep
//! in sync with the tree.

mod build;
mod deck;
mod discover;
mod edit;
mod inspect;
mod intents;
mod live;
mod run;
mod savefile;
mod shot;
mod swap;

use anyhow::{bail, Result};
use discover::GameSrc;
use std::path::PathBuf;

const USAGE: &str = "\
shinra — the Shinra Engine runner

  shinra build <game> [options]     compile contract.wit and every module to .wasm
  shinra run   <game|deck> [opts]   build, then run it in the IDE
                                    a directory of games is a deck; n moves through it
  shinra shot  <game> [options]     render headless, write a PPM to stdout

options
  -o, --out <dir>     where the .wasm files go  (default target/shinra/<name>)
      --game <name>   which game/*.rs drives    (default: the first one)
      --render <name> which render/*.rs draws   (default: the first one)
      --process <n>   which process/*.rs runs   (default: the first one)
      --release       optimise modules
  -n, --frames <N>    how many frames to render (default 1)
      --size <WxH>    framebuffer               (default 320x180)
      --save <file>   shot: write a save after the last frame
      --load <file>   shot: resume from a save, frames continue from its counter
";

pub struct Args {
    pub cmd: String,
    pub root: PathBuf,
    pub out: Option<PathBuf>,
    pub game: Option<String>,
    pub render: Option<String>,
    pub process: Option<String>,
    pub profile: String,
    pub frames: u32,
    pub size: (u32, u32),
    pub save: Option<PathBuf>,
    pub load: Option<PathBuf>,
}

fn parse() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_default();
    if cmd.is_empty() || cmd == "-h" || cmd == "--help" {
        print!("{USAGE}");
        std::process::exit(0);
    }
    let root = PathBuf::from(it.next().unwrap_or_default());
    let mut a = Args {
        cmd,
        root,
        out: None,
        game: None,
        render: None,
        process: None,
        profile: "debug".into(),
        frames: 1,
        size: (320, 180),
        save: None,
        load: None,
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-o" | "--out" => a.out = it.next().map(PathBuf::from),
            "--game" => a.game = it.next(),
            "--render" => a.render = it.next(),
            "--process" => a.process = it.next(),
            "--release" => a.profile = "release".into(),
            "-n" | "--frames" => a.frames = it.next().unwrap_or_default().parse().unwrap_or(1),
            "--save" => a.save = it.next().map(PathBuf::from),
            "--load" => a.load = it.next().map(PathBuf::from),
            "--size" => {
                let s = it.next().unwrap_or_default();
                let (w, h) = s.split_once('x').unwrap_or(("320", "180"));
                a.size = (w.parse().unwrap_or(320), h.parse().unwrap_or(180));
            }
            other => bail!("unexpected argument `{other}`\n\n{USAGE}"),
        }
    }
    if a.root.as_os_str().is_empty() {
        bail!("which game?\n\n{USAGE}");
    }
    Ok(a)
}

fn out_dir(a: &Args, src: &GameSrc) -> PathBuf {
    a.out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/shinra").join(&src.name))
}

fn main() -> Result<()> {
    let a = parse()?;
    if a.cmd == "run" {
        let deck = deck::Deck::open(&a.root)?;
        return run::run(
            deck,
            run::Options {
                profile: a.profile.clone(),
                frames: if a.frames > 1 { Some(a.frames as u64) } else { None },
                game: a.game.clone(),
                render: a.render.clone(),
            },
        );
    }
    let src = GameSrc::open(&a.root)?;
    let out = out_dir(&a, &src);

    match a.cmd.as_str() {
        "build" => {
            let b = build::build(&src, &out, &a.profile)?;
            println!("{} -> {}", src.contract.display(), b.contract.display());
            for (cat, name, path) in &b.modules {
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                println!("  {cat}/{name}.wasm  {size} B");
            }
            Ok(())
        }
        "shot" => shot::run(&a, &src, &out),
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}
