//! One game, standing: the Vm, the pipeline built for what it draws, and the
//! way to stand one up from a source tree.

use crate::build;
use crate::deck::Deck;
use crate::discover::GameSrc;
use crate::run::Options;
use anyhow::{bail, Context, Result};
use se_gpu::Gpu;
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

/// A Gpu from a render module's `se.wgsl` section.
pub fn make_gpu(vm: &Vm, out: &Path, render_bytes: &[u8], size: (u32, u32)) -> Result<Gpu> {
    let wgsl = std::str::from_utf8(
        se_contract::wasm::section(render_bytes, "se.wgsl")
            .context("the render module carries no se.wgsl section")?,
    )?;
    let contract_wgsl = std::fs::read_to_string(out.join("contract.wgsl"))?;
    Gpu::new(&vm.meta, &format!("{contract_wgsl}\n{wgsl}"), size.0, size.1)
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

    // An editor opens frozen, but it must not open uninitialised. Games create
    // their roster, camera and initial control state in tick(0); leaving that
    // until Play makes an Edit-mode viewport show only a clear (or an empty
    // pool). Run that single frame while standing the game up, then the run
    // loop can stay in Edit without advancing the simulation.
    vm.frame(0).context("initializing game frame 0")?;

    if !vm.stages().is_empty() {
        let n: Vec<&str> = vm.stages().iter().map(|s| s.name.as_str()).collect();
        log.push(format!("stages: {}", n.join(", ")));
    }

    let (rname, rpath) = pick(src, &out, "render", opt.render.as_deref()).expect("checked");
    let gpu = make_gpu(&vm, &out, &std::fs::read(rpath)?, size)
        .with_context(|| format!("render/{rname}.wasm"))?;
    let mut gpu = gpu;
    load_assets(&mut vm, &mut gpu, src, &out, &mut |l| log.push(l))?;
    Ok(Live { vm, gpu })
}

/// Hand one asset module's payload to where its kind lives: meshes and images
/// to the GPU, anything else into the arena as a blob. Both are keyed by id.
pub fn load_asset(vm: &mut Vm, gpu: &mut Gpu, id: u32, name: &str, wasm: &Path) -> Result<String> {
    let bytes = std::fs::read(wasm)?;
    vm.check_contract(&format!("asset/{name}"), &bytes)?;
    let payload = se_contract::wasm::section(&bytes, "se.asset")
        .with_context(|| format!("asset/{name} carries no se.asset section"))?;
    let kind = se_contract::wasm::section(&bytes, "se.asset.kind")
        .ok()
        .and_then(|k| std::str::from_utf8(k).ok())
        .unwrap_or("");
    let n = payload.len();
    Ok(match se_gpu::resources::decode(kind, payload).with_context(|| format!("asset/{name}"))? {
        Some(r) => {
            gpu.set_resource(id, r);
            format!("asset {name}: slot {id}, {kind} (gpu), {n} B")
        }
        None => {
            vm.put_blob(id, payload)?;
            format!("asset {name}: slot {id}, {kind}, {n} B")
        }
    })
}

/// Every asset of the game, by id (its place among the sorted names).
pub fn load_assets(
    vm: &mut Vm,
    gpu: &mut Gpu,
    src: &GameSrc,
    out: &Path,
    log: &mut dyn FnMut(String),
) -> Result<()> {
    for (i, name) in src.asset_ids().iter().enumerate() {
        let wasm = out.join("asset").join(format!("{name}.wasm"));
        log(load_asset(vm, gpu, i as u32, name, &wasm)?);
    }
    Ok(())
}
