//! `check_layout` against contracts that drifted from what this engine reads.
//!
//! A game that spelled a command field differently would be replayed at the
//! wrong offsets. The guard must refuse it, naming the field, and `Gpu::new`
//! must refuse before it asks for an adapter, so none of this needs a GPU.

use anyhow::Result;
use se_gpu::{command, Gpu};
use se_vm::world::Meta;
use se_vm::Vm;
use std::path::PathBuf;

fn game1() -> Result<String> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    Ok(std::fs::read_to_string(wit)?)
}

/// `game1.wit` with `from` replaced by `to`, generated from a temp dir.
fn meta_with(name: &str, from: &str, to: &str) -> Result<Meta> {
    let src = game1()?;
    assert!(src.contains(from), "game1.wit no longer contains `{from}`");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("layout-{name}"));
    std::fs::create_dir_all(&dir)?;
    let wit = dir.join("game.wit");
    std::fs::write(&wit, src.replacen(from, to, 1))?;
    let g = se_contract::generate(&wit)?;
    Ok(Vm::new(&g.module, 8)?.meta)
}

/// Both guards must refuse with a message containing `want`.
fn refused(name: &str, from: &str, to: &str, want: &str) -> Result<()> {
    let meta = meta_with(name, from, to)?;
    let e = command::check_layout(&meta).expect_err("check_layout accepted a drifted contract");
    assert!(e.to_string().contains(want), "check_layout: {e}");
    let e = Gpu::new(&meta, "", 0, 8, 8).err().expect("Gpu::new accepted a drifted contract");
    assert!(e.to_string().contains(want), "Gpu::new: {e}");
    Ok(())
}

#[test]
fn the_shipped_contract_passes() -> Result<()> {
    let meta = meta_with("none", "pad: u32,", "pad: u32,")?;
    command::check_layout(&meta)
}

#[test]
fn a_renamed_field_is_refused_by_name() -> Result<()> {
    refused("rename", "uniform-off:", "uniform-offset:", "uniform_off")
}

#[test]
fn a_dropped_pad_changes_the_size() -> Result<()> {
    refused("nopad", "    pad: u32,\n", "", "reads 48")
}

#[test]
fn swapped_fields_are_at_the_wrong_offsets() -> Result<()> {
    refused("swap", "vertices: u32,\n    shader: u32,", "shader: u32,\n    vertices: u32,", "is at")
}

#[test]
fn a_missing_frame_record_is_refused() -> Result<()> {
    refused(
        "noframe",
        "record frame { commands: u32, count: u32, present: u32, epoch: u32 }",
        "",
        "render.frame",
    )
}
