//! `Gpu` unit tests, split out of `lib.rs` to keep it under the project's
//! per-file line limit.

use super::*;
use std::path::PathBuf;

fn gpu(w: u32, h: u32) -> Result<Gpu> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let vm = se_vm::Vm::new(&g.module, 1)?;
    Gpu::new(&vm.meta, &g.wgsl, w, h)
}

#[test]
fn target_view_resolves_caches_and_drops_on_resize() -> Result<()> {
    let mut g = gpu(16, 16)?;

    let present = g.target_view(0).clone();
    assert_eq!(present, g.target.view);

    let first = g.target_view(5).clone();
    assert_ne!(first, present, "an offscreen target is not the present one");
    assert_eq!((g.offscreen[&5].width, g.offscreen[&5].height), (16, 16));

    let second = g.target_view(5).clone();
    assert_eq!(first, second, "a second resolve must reuse the cached target");

    g.resize(32, 32);
    let third = g.target_view(5).clone();
    assert_ne!(third, first, "resize must drop the stale-sized offscreen target");
    assert_eq!((g.offscreen[&5].width, g.offscreen[&5].height), (32, 32));
    Ok(())
}

#[test]
fn ensure_offscreen_prev_view_is_separate_from_cur_and_cached() -> Result<()> {
    let mut g = gpu(8, 8)?;

    let cur = g.target_view(2).clone();
    g.ensure_offscreen(2);
    let prev = g.offscreen[&2].prev_view.clone();
    assert_ne!(cur, prev, "FLAG_PREV must read a distinct texture from this frame's CUR");

    g.ensure_offscreen(2);
    let prev_again = g.offscreen[&2].prev_view.clone();
    assert_eq!(prev, prev_again, "a second ensure must reuse the same history pair");
    Ok(())
}
