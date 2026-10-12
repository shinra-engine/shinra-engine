//! `Gpu::replay`'s error contract and report, on hand-built command lists.
//!
//! No rustc and no wasm module: the contract alone gives a `Vm`, and a
//! `Command` is a plain struct. Needs a WebGPU adapter (lavapipe in Docker).

use anyhow::Result;
use se_gpu::command::{Command, BEGIN_PASS, DRAW_FULLSCREEN, DRAW_INSTANCED, END_PASS};
use se_gpu::Gpu;
use se_vm::Vm;
use std::path::PathBuf;

/// `TransformInstance` and `Camera` come from the prepended `contract.wgsl`.
const SHADER: &str = r#"@group(0) @binding(0) var<uniform> cam: Camera;
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) tint: vec3<f32> };
@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: TransformInstance) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-0.6, -0.5), vec2<f32>(0.6, -0.5), vec2<f32>(0.0, 0.6));
    let c = corners[vi] * clamp(inst.scale.x, 0.02, 1.0);
    var o: VsOut;
    o.clip = vec4<f32>(vec2<f32>(inst.pos.x, inst.pos.y) + c, 0.0, 1.0);
    o.tint = vec3<f32>(clamp(inst.scale.x * 2.0, 0.0, 1.0), cam.fov, inst.rot.w);
    return o;
}
@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> { return vec4<f32>(i.tint, 1.0); }
"#;

struct Rig {
    vm: Vm,
    gpu: Gpu,
    column: u32,
}

fn rig() -> Result<Rig> {
    rig_sized(64, 64)
}

fn rig_sized(w: u32, h: u32) -> Result<Rig> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let column = g
        .contract
        .components()
        .iter()
        .position(|r| r.name == "transform")
        .unwrap() as u32;
    let vm = Vm::new(&g.module, 128)?;
    let gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), column, w, h)?;
    Ok(Rig { vm, gpu, column })
}

fn cmd(kind: u32) -> Command {
    Command {
        kind,
        vertices: 0,
        shader: 0,
        column: u32::MAX,
        count: 0,
        uniform_off: 0,
        uniform_len: 0,
        clear: [0.0, 0.0, 0.0, 1.0],
    }
}

fn replay(r: &Rig, cmds: &[Command]) -> Result<se_gpu::replay::Report> {
    r.gpu.replay(&r.vm.meta, &r.vm.world, r.vm.bytes(), cmds)
}

fn refused(r: &Rig, cmds: &[Command], want: &str) {
    let e = replay(r, cmds).expect_err("the frame should be refused");
    assert!(format!("{e:#}").contains(want), "`{e:#}` should mention `{want}`");
}

#[test]
fn an_unknown_command_kind_is_refused() -> Result<()> {
    let r = rig()?;
    refused(&r, &[cmd(BEGIN_PASS), cmd(7)], "no rule");
    Ok(())
}

#[test]
fn a_frame_that_opens_no_pass_is_refused() -> Result<()> {
    let r = rig()?;
    refused(&r, &[], "opens no pass");
    Ok(())
}

#[test]
fn a_draw_of_the_wrong_column_is_refused() -> Result<()> {
    let r = rig()?;
    let draw = Command { column: r.column + 1, count: 1, ..cmd(DRAW_INSTANCED) };
    refused(&r, &[cmd(BEGIN_PASS), draw, cmd(END_PASS)], "built for column");
    Ok(())
}

#[test]
fn a_column_past_the_memory_is_refused() -> Result<()> {
    let r = rig()?;
    let draw = Command { column: r.column, count: u32::MAX, ..cmd(DRAW_INSTANCED) };
    refused(
        &r,
        &[cmd(BEGIN_PASS), draw, cmd(END_PASS)],
        "column a draw names runs past the memory",
    );
    Ok(())
}

#[test]
fn a_uniform_past_the_memory_is_refused() -> Result<()> {
    let r = rig()?;
    let end = r.vm.bytes().len() as u32;
    let draw = Command { uniform_off: end - 16, uniform_len: 64, ..cmd(DRAW_FULLSCREEN) };
    refused(
        &r,
        &[cmd(BEGIN_PASS), draw, cmd(END_PASS)],
        "uniform a draw names runs past",
    );
    Ok(())
}

#[test]
fn a_valid_frame_reports_what_it_did() -> Result<()> {
    let r = rig()?;
    let clear = [0.25, 0.5, 0.75, 1.0];
    let begin = Command { clear, ..cmd(BEGIN_PASS) };
    let draw = Command { column: r.column, count: 2, ..cmd(DRAW_INSTANCED) };
    let report = replay(&r, &[begin, draw, cmd(END_PASS)])?;
    assert_eq!(
        report,
        se_gpu::replay::Report { passes: 1, draws: 1, instances: 2, clear }
    );
    Ok(())
}

#[test]
fn clear_only_frame_is_flat() -> Result<()> {
    // 64 wide has no row padding; 50 wide (200 of 256 bytes) needs it stripped.
    for (w, h) in [(64u32, 64u32), (50, 10)] {
        let r = rig_sized(w, h)?;
        let begin = Command { clear: [0.2, 0.4, 0.6, 1.0], ..cmd(BEGIN_PASS) };
        replay(&r, &[begin, cmd(END_PASS)])?;
        let px = r.gpu.pixels()?;
        assert_eq!(px.len(), (w * h * 4) as usize, "{w}x{h} length");
        assert!(
            px.chunks(4).all(|t| t == [51, 102, 153, 255]),
            "{w}x{h} should be flat (51,102,153,255)"
        );
    }
    Ok(())
}
