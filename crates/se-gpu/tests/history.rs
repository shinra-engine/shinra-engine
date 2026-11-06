//! task-264: a `FLAG_PREV` draw reads a target's *previous* frame, not
//! whatever that target holds mid-encode this frame — hand-built against
//! `se-gpu` directly, no example game involved.

use anyhow::Result;
use se_gpu::command::{Command, BEGIN_PASS, DRAW_FULLSCREEN, END_PASS, FLAG_PREV, NONE};
use se_gpu::Gpu;
use se_vm::Vm;
use std::path::PathBuf;

/// Same fullscreen-triangle vertex shader as texture_draw.rs/offscreen.rs:
/// no instance input, so one pipeline serves every `DRAW_FULLSCREEN` here,
/// whatever view `bind_for` picks for binding 1.
const SHADER: &str = r#"
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: VsOut;
    o.clip = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}
@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> { return textureSample(se_tex, se_samp, i.uv); }
"#;

fn cmd(kind: u32) -> Command {
    Command {
        kind,
        vertices: 0,
        shader: 0,
        column: u32::MAX,
        count: 0,
        uniform_off: 0,
        uniform_len: 0,
        mesh: u32::MAX,
        texture: u32::MAX,
        clear: [0.0, 0.0, 0.0, 1.0],
        target: 0,
        sample: NONE,
        render_flags: 0,
    }
}

fn rig(w: u32, h: u32) -> Result<(Vm, Gpu)> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let vm = Vm::new(&g.module, 128)?;
    let gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), w, h)?;
    Ok((vm, gpu))
}

fn first_pixel(px: &[u8]) -> [u8; 4] {
    [px[0], px[1], px[2], px[3]]
}

#[test]
fn flag_prev_reads_the_target_as_it_was_at_the_end_of_the_previous_frame() -> Result<()> {
    let (vm, mut gpu) = rig(16, 16)?;

    // Frame 1: target 2 gets a solid, distinct colour. No present draw —
    // only target 2's `cur` matters for this frame.
    let frame1_target = Command { target: 2, clear: [0.0, 1.0, 0.0, 1.0], ..cmd(BEGIN_PASS) };
    gpu.replay(&vm.meta, &vm.world, vm.bytes(), &[frame1_target, cmd(END_PASS)])?;

    // Frame 2: present pass draws a FULLSCREEN with sample=2, FLAG_PREV.
    // Target 2's `cur` is untouched this frame (never re-opened), so if the
    // draw read `cur` instead of `prev` it would see frame 1's content too —
    // the real test is frame 3 below, where `cur` and `prev` disagree.
    let frame2_present = Command { target: 0, clear: [1.0, 1.0, 1.0, 1.0], ..cmd(BEGIN_PASS) };
    let read_prev =
        Command { kind: DRAW_FULLSCREEN, sample: 2, render_flags: FLAG_PREV, ..cmd(0) };
    gpu.replay(
        &vm.meta,
        &vm.world,
        vm.bytes(),
        &[frame2_present, read_prev, cmd(END_PASS)],
    )?;
    let px = gpu.pixels()?;
    assert_eq!(
        first_pixel(&px),
        [0, 255, 0, 255],
        "FLAG_PREV must bind target 2's history, carrying frame 1's colour forward"
    );

    // Frame 3: target 2 is re-painted a different colour in the SAME frame
    // that reads its PREV — proving the read is the history texture, not
    // `cur` mid-encode. `prev` must still show frame 2's content: target 2
    // was never drawn in frame 2, so `prev` still holds frame 1's green.
    let frame3_target = Command { target: 2, clear: [0.0, 0.0, 1.0, 1.0], ..cmd(BEGIN_PASS) };
    let frame3_present = Command { target: 0, clear: [1.0, 1.0, 1.0, 1.0], ..cmd(BEGIN_PASS) };
    gpu.replay(
        &vm.meta,
        &vm.world,
        vm.bytes(),
        &[
            frame3_target,
            cmd(END_PASS),
            frame3_present,
            read_prev,
            cmd(END_PASS),
        ],
    )?;
    let px = gpu.pixels()?;
    assert_eq!(
        first_pixel(&px),
        [0, 255, 0, 255],
        "reading PREV in the same frame target 2 is repainted must still see the OLD prev, \
         not this frame's in-progress cur"
    );
    Ok(())
}

#[test]
fn flag_prev_on_a_target_never_written_reads_zero_not_a_bail() -> Result<()> {
    let (vm, mut gpu) = rig(16, 16)?;

    // The fullscreen pipeline alpha-blends; a zero-alpha source leaves the
    // pass's clear showing through untouched, same as "no draw happened"
    // would. A distinctive, non-white, non-zero clear tells that apart from
    // a wrongly-bound opaque `self.white` (which would overwrite it).
    let present = Command { target: 0, clear: [0.2, 0.4, 0.6, 1.0], ..cmd(BEGIN_PASS) };
    let read_prev =
        Command { kind: DRAW_FULLSCREEN, sample: 7, render_flags: FLAG_PREV, ..cmd(0) };
    gpu.replay(&vm.meta, &vm.world, vm.bytes(), &[present, read_prev, cmd(END_PASS)])?;
    let px = gpu.pixels()?;
    let got = first_pixel(&px);
    let want = [51, 102, 153, 255];
    let near = got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 16);
    assert!(
        near,
        "target 7 was never written by any earlier frame; its zero-alpha history must let the \
         pass's clear {want:?} show through untouched, got {got:?} (255 would mean the opaque \
         `self.white` default was bound by mistake)"
    );
    Ok(())
}

#[test]
fn flag_prev_without_a_sample_target_is_refused() -> Result<()> {
    let (vm, mut gpu) = rig(16, 16)?;
    let bad = Command { kind: DRAW_FULLSCREEN, render_flags: FLAG_PREV, ..cmd(0) };
    let e = gpu
        .replay(&vm.meta, &vm.world, vm.bytes(), &[cmd(BEGIN_PASS), bad, cmd(END_PASS)])
        .unwrap_err();
    assert!(format!("{e:#}").contains("FLAG_PREV"), "{e:#}");
    Ok(())
}

#[test]
fn flag_prev_naming_the_present_target_is_refused() -> Result<()> {
    let (vm, mut gpu) = rig(16, 16)?;
    let bad = Command { kind: DRAW_FULLSCREEN, sample: 0, render_flags: FLAG_PREV, ..cmd(0) };
    let e = gpu
        .replay(&vm.meta, &vm.world, vm.bytes(), &[cmd(BEGIN_PASS), bad, cmd(END_PASS)])
        .unwrap_err();
    assert!(format!("{e:#}").contains("FLAG_PREV"), "{e:#}");
    Ok(())
}
