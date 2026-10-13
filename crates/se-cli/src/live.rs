//! One game, standing: the Vm, the pipeline built for what it draws, and the
//! way to stand one up from a source tree.

use crate::build;
use crate::deck::Deck;
use crate::discover::GameSrc;
use crate::run::Options;
use anyhow::{bail, Context, Result};
use se_gpu::{command, Gpu};
use se_vm::memory::Slot;
use se_vm::Vm;
use std::path::{Path, PathBuf};

/// Rows per pool. One number: a pool is homogeneous by construction, so its
/// columns cannot want different capacities.
const ROWS: u32 = 1024;

/// One game, standing.
pub struct Live {
    pub vm: Vm,
    pub gpu: Gpu,
    /// The column the recorded frame draws, which is what the pipeline was
    /// built for.
    pub column: u32,
}

pub fn pick(src: &GameSrc, out: &Path, cat: &str, want: Option<&str>) -> Option<(String, PathBuf)> {
    let c = src.category(cat)?;
    if build::is_additive(cat) {
        if c.modules.is_empty() {
            return None;
        }
        return Some((cat.to_string(), out.join(cat).join(format!("{cat}.wasm"))));
    }
    let m = match want {
        Some(n) => c.modules.iter().find(|m| m.name == n)?,
        None => c.modules.first()?,
    };
    Some((m.name.clone(), out.join(cat).join(format!("{}.wasm", m.name))))
}

/// A pipeline from a render module's `se.wgsl` section, for the column it draws.
pub fn make_gpu(vm: &Vm, out: &Path, render_bytes: &[u8], column: u32, size: (u32, u32)) -> Result<Gpu> {
    let wgsl = std::str::from_utf8(
        se_contract::wasm::section(render_bytes, "se.wgsl")
            .context("the render module carries no se.wgsl section")?,
    )?;
    let contract_wgsl = std::fs::read_to_string(out.join("contract.wgsl"))?;
    Gpu::new(&vm.meta, &format!("{contract_wgsl}\n{wgsl}"), column, size.0, size.1)
}

/// Build whichever game the deck points at, stand it up, and make a pipeline
/// for the column it turns out to draw.
pub fn open(deck: &Deck, opt: &Options, size: (u32, u32), log: &mut Vec<String>) -> Result<Live> {
    let src = deck.src();
    let out = Path::new("target").join("shinra").join(&src.name);
    build::build(src, &out, &opt.profile)?;

    let mut vm = Vm::new(&std::fs::read(out.join("contract.wasm"))?, ROWS)?;
    for (slot, cat, want) in [
        (Slot::Game, "game", opt.game.as_deref()),
        (Slot::Process, "process", None),
        (Slot::Render, "render", opt.render.as_deref()),
    ] {
        let Some((name, path)) = pick(src, &out, cat, want) else {
            bail!("`{}` has no {cat}/*.rs to fill the {cat} slot", src.name)
        };
        vm.load(slot, &name, std::fs::read(&path)?)
            .with_context(|| format!("loading {cat}/{name}.wasm"))?;
    }
    if !vm.stages().is_empty() {
        let n: Vec<&str> = vm.stages().iter().map(|s| s.name.as_str()).collect();
        log.push(format!("stages: {}", n.join(", ")));
    }

    // Assets are data: the bytes ride in a custom section and never need
    // instantiating. A row names one with a `span`, so swapping a 12-byte mesh
    // for an 800-byte one changes two u32s and no layout.
    for (i, name) in src.asset_ids().iter().enumerate() {
        let bytes = std::fs::read(out.join("asset").join(format!("{name}.wasm")))?;
        vm.check_contract(&format!("asset/{name}"), &bytes)?;
        if let Ok(payload) = se_contract::wasm::section(&bytes, "se.asset") {
            vm.put_blob(i as u32, payload)?;
            log.push(format!("asset {name}: slot {i}, {} B", payload.len()));
        }
    }

    // One frame first, so the module says what it wants drawn rather than the
    // host deciding for it.
    vm.frame(0)?;
    let (cmds, _) = command::read(&vm.meta, vm.bytes())?;
    let draw = cmds
        .iter()
        .find(|c| c.kind == command::DRAW_INSTANCED)
        .context("the recorded frame has no instanced draw to build a pipeline for")?;
    let column = draw.column;

    let (rname, rpath) = pick(src, &out, "render", opt.render.as_deref()).expect("checked");
    let gpu = make_gpu(&vm, &out, &std::fs::read(rpath)?, column, size)
        .with_context(|| format!("render/{rname}.wasm"))?;
    Ok(Live { vm, gpu, column })
}
