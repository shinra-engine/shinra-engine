//! `se-contract <contract.wit> [-o <dir>]`
//!
//! Writes `contract.rs` and `contract.wgsl` beside each other, and prints the
//! layout it chose. The print is not decoration: the offsets it shows are the
//! ones the shader will use, and reading them is how an author checks that a
//! field landed where they meant it to.

use anyhow::{bail, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut wit: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--out" => out = it.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("se-contract <contract.wit> [-o <dir>]");
                return Ok(());
            }
            _ if wit.is_none() => wit = Some(PathBuf::from(a)),
            other => bail!("unexpected argument `{other}`"),
        }
    }
    let Some(wit) = wit else {
        bail!("usage: se-contract <contract.wit> [-o <dir>]")
    };
    let dir = out.unwrap_or_else(|| wit.parent().unwrap_or(std::path::Path::new(".")).to_path_buf());

    let g = se_contract::generate(&wit)?;

    println!("std430 layout from {}:\n", wit.display());
    for r in &g.contract.records {
        println!("  {}::{}  size {} align {}", r.iface, r.name, r.size, r.align);
        for f in &r.fields {
            println!("      @{:<4} {:<12} {:?}", f.offset, f.name, f.ty);
        }
    }
    println!("\npools:");
    for p in &g.contract.pools {
        println!("  {}  <- {}", p.name, p.columns.join(", "));
    }
    println!("\narena (BASE must equal every module's -z stack-size):");
    println!("  0x{:08x}  base", g.map.base);
    for r in &g.map.control {
        println!("  0x{:08x}  control::{} ({} B)", r.at, r.name, r.len);
    }
    println!("  0x{:08x}  render::frame", g.map.frame);
    println!("  0x{:08x}  command buffer", g.map.commands);
    println!("  0x{:08x}  pool table", g.map.pools);
    println!("  0x{:08x}  column table", g.map.columns);
    println!("  0x{:08x}  blob table", g.map.blobs);
    println!("  0x{:08x}  heap", g.map.heap);

    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("contract.rs"), &g.rust)?;
    std::fs::write(dir.join("contract.wgsl"), &g.wgsl)?;
    std::fs::write(dir.join("contract.wasm"), &g.module)?;
    println!("\nwrote:");
    println!("  contract.rs    {:>6} B   modules and any host built for one game", g.rust.len());
    println!("  contract.wgsl  {:>6} B   shaders", g.wgsl.len());
    println!("  contract.wasm  {:>6} B   the table a game-agnostic host reads", g.module.len());
    Ok(())
}
