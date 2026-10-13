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

/// The shipped contract's meta, with the render records drifted by `drift`.
/// `render.*` now comes from the `shinra:engine` package, which a game cannot
/// edit, so the drift is applied to the decoded table instead.
fn meta_with(drift: impl FnOnce(&mut Meta)) -> Result<Meta> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let mut meta = Vm::new(&g.module, 8)?.meta;
    drift(&mut meta);
    Ok(meta)
}

fn rec<'a>(m: &'a mut Meta, name: &str) -> &'a mut se_contract::read::RecordInfo {
    m.all.iter_mut().find(|r| r.name == name).expect("record")
}

/// Both guards must refuse with a message containing `want`.
fn refused(drift: impl FnOnce(&mut Meta), want: &str) -> Result<()> {
    let meta = meta_with(drift)?;
    let e = command::check_layout(&meta).expect_err("check_layout accepted a drifted contract");
    assert!(e.to_string().contains(want), "check_layout: {e}");
    let e = Gpu::new(&meta, "", 0, 8, 8).err().expect("Gpu::new accepted a drifted contract");
    assert!(e.to_string().contains(want), "Gpu::new: {e}");
    Ok(())
}

#[test]
fn the_shipped_contract_passes() -> Result<()> {
    let meta = meta_with(|_| {})?;
    command::check_layout(&meta)
}

#[test]
fn a_renamed_field_is_refused_by_name() -> Result<()> {
    refused(|m| {
        let f = rec(m, "command").fields.iter_mut().find(|f| f.name == "uniform-off").unwrap();
        f.name = "uniform-offset".into();
    }, "uniform_off")
}

#[test]
fn a_dropped_pad_changes_the_size() -> Result<()> {
    refused(|m| {
        let r = rec(m, "command");
        r.fields.retain(|f| f.name != "pad");
        r.size = 44;
    }, "reads 48")
}

#[test]
fn swapped_fields_are_at_the_wrong_offsets() -> Result<()> {
    refused(|m| {
        for f in rec(m, "command").fields.iter_mut() {
            match f.name.as_str() {
                "vertices" => f.offset = 8,
                "shader" => f.offset = 4,
                _ => {}
            }
        }
    }, "is at")
}

#[test]
fn a_missing_frame_record_is_refused() -> Result<()> {
    refused(|m| m.all.retain(|r| r.name != "frame"), "render.frame")
}
