//! Replace the game or render module of a running Vm.
//!
//! The world is not in a module, so nothing here touches a row. A render swap
//! also rebuilds the pipeline, because the shader is the module's own
//! `se.wgsl` section; if any step fails the previous module is loaded back and
//! the old pipeline is never dropped, so the scene keeps drawing.

use crate::discover::GameSrc;
use crate::live::{make_gpu, pick, Live};
use anyhow::{bail, Context, Result};
use se_vm::memory::Slot;
use std::path::Path;

pub fn swap(live: &mut Live, src: &GameSrc, out: &Path, cat: &str, name: &str, size: (u32, u32)) -> Result<String> {
    let slot = match cat {
        "game" => Slot::Game,
        "render" => Slot::Render,
        _ => bail!("only game and render can be swapped, not {cat}"),
    };
    let Some((name, path)) = pick(src, out, cat, Some(name)) else {
        bail!("no {cat}/{name}.rs in `{}`", src.name)
    };
    let bytes = std::fs::read(&path).with_context(|| format!("reading {cat}/{name}.wasm"))?;
    let prev = live.vm.module(slot).map(|m| (m.name.clone(), m.bytes.clone()));

    let tried = live.vm.load(slot, &name, bytes.clone());
    let gpu = tried.and_then(|_| {
        if slot == Slot::Game {
            return Ok(None);
        }
        let mut g = make_gpu(&live.vm, out, &bytes, size)?;
        // A fresh Gpu holds no meshes or images; upload them again by id.
        crate::live::load_assets(&mut live.vm, &mut g, src, out, &mut |_| {})?;
        Ok(Some(g))
    });
    match gpu {
        Ok(Some(g)) => live.gpu = g,
        Ok(None) => {}
        Err(e) => {
            if let Some((pn, pb)) = prev {
                let _ = live.vm.load(slot, &pn, pb);
            }
            return Err(e.context(format!("swapping to {cat}/{name}")));
        }
    }
    Ok(format!("{cat}: {name}"))
}

/// Enter on the Project panel: swap in the module under the cursor and say how
/// it went in the log. A heading or any other row carries no id and does nothing.
pub fn swap_selected(
    ide: &mut se_tui::Ide,
    live: &mut Live,
    src: &GameSrc,
    ids: &[Option<(String, String)>],
    size: (u32, u32),
) {
    let Some(Some((cat, name))) = ids.get(ide.project_sel) else { return };
    let out = Path::new("target").join("shinra").join(&src.name);
    match swap(live, src, &out, cat, name, size) {
        Ok(_) => ide.note(format!("swapped {cat} -> {name}")),
        Err(e) => ide.note(format!("swap failed: {e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{deck::Deck, run::Options};
    use se_gpu::command;

    fn deck() -> Deck {
        let root = std::env::var("SHINRA_EXAMPLES")
            .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../shinra-examples").into());
        Deck::open(&Path::new(&root).join("game1")).expect("game1")
    }

    fn opts() -> Options {
        Options { profile: "debug".into(), frames: None, game: None, render: Some("solid".into()), watch: false, pad: None }
    }

    #[test]
    fn render_solid_to_ghost_keeps_world_and_bad_name_errs() {
        let d = deck();
        let mut live = crate::live::open(&d, &opts(), (64, 36), &mut Vec::new()).unwrap();
        let out = Path::new("target").join("shinra").join(&d.src().name);
        // The command list lives at the arena's head and is rewritten by any record.
        let rows = |l: &Live| l.vm.snapshot()[(l.vm.meta.pools_at - l.vm.meta.base) as usize..].to_vec();
        live.vm.frame(0).unwrap();
        let before = rows(&live);
        let (cmds, _) = command::read(&live.vm.meta, live.vm.bytes()).unwrap();
        live.gpu.replay(&live.vm.meta, &live.vm.world, live.vm.bytes(), &cmds).unwrap();
        let first = live.gpu.pixels().ok();
        assert_eq!(live.vm.module(Slot::Render).unwrap().name, "solid");

        assert!(swap(&mut live, d.src(), &out, "render", "nope", (64, 36)).is_err());
        assert_eq!(live.vm.module(Slot::Render).unwrap().name, "solid");

        swap(&mut live, d.src(), &out, "render", "ghost", (64, 36)).unwrap();
        assert_eq!(live.vm.module(Slot::Render).unwrap().name, "ghost");
        assert_eq!(rows(&live), before);
        live.vm.record_only().unwrap();
        let (cmds, _) = command::read(&live.vm.meta, live.vm.bytes()).unwrap();
        live.gpu.replay(&live.vm.meta, &live.vm.world, live.vm.bytes(), &cmds).unwrap();
        assert_ne!(live.gpu.pixels().ok(), first);
    }
}
