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
use crate::Args;
use anyhow::{bail, Context, Result};
use se_gpu::{command, Gpu};
use se_vm::memory::Slot;
use se_vm::Vm;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Which artifact fills a slot. `process/` is additive, so its one module is
/// named after the category; every other category is a choice between
/// alternatives, and the name is the file's.
fn pick(
    src: &GameSrc,
    out: &Path,
    category: &str,
    want: Option<&str>,
) -> Option<(String, PathBuf)> {
    let cat = src.category(category)?;
    if crate::build::is_additive(category) {
        if cat.modules.is_empty() {
            return None;
        }
        let n: Vec<&str> = cat.modules.iter().map(|m| m.name.as_str()).collect();
        return Some((n.join("+"), out.join(category).join(format!("{category}.wasm"))));
    }
    let m = match want {
        Some(n) => cat.modules.iter().find(|m| m.name == n)?,
        None => cat.modules.first()?,
    };
    Some((m.name.clone(), out.join(category).join(format!("{}.wasm", m.name))))
}

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
        let Some((name, path)) = pick(src, out, category, want) else {
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

    // Assets are data: their bytes ride in a custom section and never need
    // instantiating. Each lands in a blob slot, and a record names it with a
    // `span` — which is why a 12-byte mesh and an 800-byte one cost the same
    // row.
    if let Some(cat) = src.category("asset") {
        for (i, m) in cat.modules.iter().enumerate() {
            let path = out.join("asset").join(format!("{}.wasm", m.name));
            let bytes = std::fs::read(&path)?;
            vm.check_contract(&format!("asset/{}", m.name), &bytes)?;
            match se_contract::wasm::section(&bytes, "se.asset") {
                Ok(payload) => {
                    let (at, len) = vm.put_blob(i as u32, payload)?;
                    eprintln!("  asset {}: slot {i} -> 0x{at:08x} +{len}", m.name);
                }
                Err(_) => eprintln!("  asset {}: no se.asset section, skipped", m.name),
            }
        }
    }

    // One frame first, so the module can say what it wants drawn.
    let (rows, n) = vm.frame(0)?;
    for o in vm.take_overruns() {
        eprintln!("  {o}");
    }
    let (cmds, _) = command::read(&vm.meta, vm.bytes())?;
    if n == 0 {
        bail!("render.wasm recorded no commands");
    }
    let Some(draw) = cmds.iter().find(|c| c.kind == command::DRAW_INSTANCED) else {
        bail!("the recorded frame has no instanced draw, so there is nothing to build a pipeline for")
    };
    let column = draw.column;
    eprintln!(
        "  frame 0: {rows} rows, {n} commands, drawing column {column} (`{}`)",
        vm.meta.components[column as usize].name
    );

    let render_bytes = {
        let (name, path) = pick(src, out, "render", a.render.as_deref()).expect("checked above");
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
    let gpu = Gpu::new(&vm.meta, &format!("{contract_wgsl}\n{wgsl}"), column, w, h)?;
    eprintln!("  adapter: {}", gpu.adapter());

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
