//! Render frames headless and write a PPM.
//!
//! The order here is the design, and it is worth reading as one thing:
//!
//! ```text
//! contract.wasm   the host reads it; it compiles no game in
//! game.wasm       writes the control var set
//! process.wasm    reads control vars and columns, writes columns
//! render.wasm     records a command list
//! host            replays it, uploading a column with no repacking
//! ```
//!
//! Note where the instance column comes from: the *recorded frame*. The host
//! runs one frame first and then builds the pipeline for the column the module
//! chose to draw, rather than deciding for it.

use crate::build;
use crate::discover::GameSrc;
use crate::pad::{PadSource, ScriptedPad};
use crate::Args;
use anyhow::{bail, Context, Result};
use se_gpu::{command, Gpu};
use se_vm::memory::Slot;
use se_vm::Vm;
use std::io::Write;
use std::path::Path;

pub fn run(a: &Args, src: &GameSrc, out: &Path) -> Result<()> {
    let built = build::build(src, out, &a.profile)?;
    let contract = std::fs::read(&built.contract)?;
    let mut vm = Vm::new(&contract, 1024)?;
    eprintln!("contract: {} components, {} pools", vm.meta.components.len(), vm.meta.pools.len());

    for (slot, category, want) in [
        (Slot::Game, "game", a.game.as_deref()),
        (Slot::Process, "process", a.process.as_deref()),
        (Slot::Render, "render", a.render.as_deref()),
    ] {
        let Some((name, path)) = crate::live::pick(src, out, category, want) else {
            bail!("`{}` has no {category}/*.rs to fill the {category} slot", src.name)
        };
        vm.load(slot, &name, std::fs::read(&path)?)
            .with_context(|| format!("loading {category}/{name}.wasm"))?;
        eprintln!("  {category}: {name}");
    }
    if !vm.stages().is_empty() {
        let names: Vec<&str> = vm.stages().iter().map(|s| s.name.as_str()).collect();
        eprintln!("  stages: {}", names.join(", "));
    }

    let mut pad_source = a.pad.as_deref().map(ScriptedPad::open).transpose()?;

    // One frame first, so the module can say what it wants drawn. No pad
    // state yet: the loop below is the real, numbered frame sequence a
    // scripted source is written against, and this probe isn't part of it.
    let (rows, n) = vm.frame(0)?;
    for o in vm.take_overruns() {
        eprintln!("  {o}");
    }
    command::read(&vm.meta, vm.bytes())?;
    if n == 0 {
        bail!("render.wasm recorded no commands");
    }
    eprintln!("  frame 0: {rows} rows, {n} commands");

    let render_bytes = {
        let (name, path) = crate::live::pick(src, out, "render", a.render.as_deref()).expect("checked above");
        let _ = name;
        std::fs::read(path)?
    };
    let wgsl = std::str::from_utf8(
        se_contract::wasm::section(&render_bytes, "se.wgsl")
            .context("the render module carries no se.wgsl section")?,
    )?
    .to_string();
    let contract_wgsl = std::fs::read_to_string(out.join("contract.wgsl"))?;

    let (w, h) = a.size;
    let mut gpu = Gpu::new(&vm.meta, &format!("{contract_wgsl}\n{wgsl}"), w, h)?;
    eprintln!("  adapter: {}", gpu.adapter());
    crate::live::load_assets(&mut vm, &mut gpu, src, out, &mut |l| eprintln!("  {l}"))?;

    let mut start = 0u32;
    if let Some(file) = &a.load {
        let (frame, _) = crate::savefile::read_from(&mut vm, file, &src.name)
            .with_context(|| format!("loading {}", file.display()))?;
        start = frame as u32;
        eprintln!("  loaded {} at frame {start}", file.display());
    }
    let end = start + a.frames.max(1);
    let mut pixels = Vec::new();
    for f in start..end {
        if let Some(ps) = pad_source.as_mut() {
            vm.set_pad(ps.poll(f))?;
        }
        let (rows, _) = vm.frame(f)?;
        for o in vm.take_overruns() {
            eprintln!("  {o}");
        }
        let (cmds, epoch) = command::read(&vm.meta, vm.bytes())?;
        let report = gpu.replay(&vm.meta, &vm.world, vm.bytes(), &cmds)?;
        pixels = gpu.pixels()?;
        eprintln!(
            "  frame {f}: rows {rows}, epoch {epoch}, {} draws, {} instances",
            report.draws, report.instances
        );
    }

    if let Some(file) = &a.save {
        let note = crate::savefile::write_to(&vm, file, end as u64)?;
        eprintln!("  {note} -> {}", file.display());
    }

    // P6: the smallest thing a viewer will open, and it costs no dependency.
    let mut o = std::io::stdout().lock();
    write!(o, "P6\n{w} {h}\n255\n")?;
    for p in pixels.chunks(4) {
        o.write_all(&p[..3])?;
    }
    o.flush()?;
    Ok(())
}
