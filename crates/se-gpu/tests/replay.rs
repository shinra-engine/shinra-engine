//! `Gpu::replay`'s error contract and report, on hand-built command lists.
//!
//! No rustc and no wasm module: the contract alone gives a `Vm`, and a
//! `Command` is a plain struct. Needs a WebGPU adapter (lavapipe in Docker).

use anyhow::Result;
use se_gpu::command::{Command, BEGIN_PASS, DRAW_INSTANCED, END_PASS};
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
    let gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), w, h)?;
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
        mesh: u32::MAX,
        texture: u32::MAX,
        clear: [0.0, 0.0, 0.0, 1.0],
    }
}

fn replay(r: &mut Rig, cmds: &[Command]) -> Result<se_gpu::replay::Report> {
    r.gpu.replay(&r.vm.meta, &r.vm.world, r.vm.bytes(), cmds)
}

fn refused(r: &mut Rig, cmds: &[Command], want: &str) {
    let e = replay(r, cmds).expect_err("the frame should be refused");
    assert!(format!("{e:#}").contains(want), "`{e:#}` should mention `{want}`");
}

#[test]
fn an_unknown_command_kind_is_refused() -> Result<()> {
    let mut r = rig()?;
    refused(&mut r, &[cmd(BEGIN_PASS), cmd(7)], "no rule");
    Ok(())
}

#[test]
fn a_frame_that_opens_no_pass_is_refused() -> Result<()> {
    let mut r = rig()?;
    refused(&mut r, &[], "opens no pass");
    Ok(())
}

#[test]
fn an_absurd_draw_count_is_refused() -> Result<()> {
    let mut r = rig()?;
    // An absurd count is refused by the capacity check, which runs before the
    // pool and memory checks; those stay as the defence behind it.
    let draw = Command { column: r.column, count: u32::MAX, ..cmd(DRAW_INSTANCED) };
    refused(
        &mut r,
        &[cmd(BEGIN_PASS), draw, cmd(END_PASS)],
        "pool holds only",
    );
    Ok(())
}

#[test]
fn a_uniform_past_the_memory_is_refused() -> Result<()> {
    let mut r = rig()?;
    let end = r.vm.bytes().len() as u32;
    let draw = Command { column: r.column, count: 1, uniform_off: end - 16, uniform_len: 64, ..cmd(DRAW_INSTANCED) };
    refused(
        &mut r,
        &[cmd(BEGIN_PASS), draw, cmd(END_PASS)],
        "uniform a draw names runs past",
    );
    Ok(())
}

#[test]
fn a_valid_frame_reports_what_it_did() -> Result<()> {
    let mut r = rig()?;
    let clear = [0.25, 0.5, 0.75, 1.0];
    let begin = Command { clear, ..cmd(BEGIN_PASS) };
    let draw = Command { column: r.column, count: 2, ..cmd(DRAW_INSTANCED) };
    let report = replay(&mut r, &[begin, draw, cmd(END_PASS)])?;
    assert_eq!(
        report,
        se_gpu::replay::Report { passes: 1, draws: 1, instances: 2, clear, pipelines: 1 }
    );
    Ok(())
}

#[test]
fn clear_only_frame_is_flat() -> Result<()> {
    // 64 wide has no row padding; 50 wide (200 of 256 bytes) needs it stripped.
    for (w, h) in [(64u32, 64u32), (50, 10)] {
        let mut r = rig_sized(w, h)?;
        let begin = Command { clear: [0.2, 0.4, 0.6, 1.0], ..cmd(BEGIN_PASS) };
        replay(&mut r, &[begin, cmd(END_PASS)])?;
        let px = r.gpu.pixels()?;
        assert_eq!(px.len(), (w * h * 4) as usize, "{w}x{h} length");
        assert!(
            px.chunks(4).all(|t| t == [51, 102, 153, 255]),
            "{w}x{h} should be flat (51,102,153,255)"
        );
    }
    Ok(())
}

fn frame(draws: &[Command]) -> Vec<Command> {
    let mut v = vec![cmd(BEGIN_PASS)];
    v.extend_from_slice(draws);
    v.push(cmd(END_PASS));
    v
}

fn draw(r: &Rig, count: u32, uniform_off: u32, uniform_len: u32) -> Command {
    Command { column: r.column, count, uniform_off, uniform_len, vertices: 3, ..cmd(DRAW_INSTANCED) }
}

/// Rows 0..3 of `transform` as triangles of scale 0.5 at the given x.
fn seed(r: &mut Rig, xs: &[f32]) -> Result<()> {
    let col = (r.vm.meta.columns_at + r.column * se_vm::world::COLUMN_BYTES) as usize;
    let u = |at: usize| u32::from_le_bytes(r.vm.bytes()[at..at + 4].try_into().unwrap());
    let (base, stride) = (u(col), u(col + 8));
    for (i, x) in xs.iter().enumerate() {
        let mut row = vec![0u8; stride as usize];
        row[..4].copy_from_slice(&x.to_le_bytes());
        // `scale` sits after pos (0) and rot (16).
        row[32..36].copy_from_slice(&0.5f32.to_le_bytes());
        r.vm.write(base + i as u32 * stride, &row)?;
    }
    Ok(())
}

#[test]
fn two_draws_of_different_counts_both_land() -> Result<()> {
    let mut r = rig()?;
    seed(&mut r, &[-0.5, 0.5, 0.0])?;
    let (one, two) = (draw(&r, 1, 0, 0), draw(&r, 2, 0, 0));
    let a = replay(&mut r, &frame(&[one]))?;
    let first = r.gpu.pixels()?;
    let b = replay(&mut r, &frame(&[one, two]))?;
    let both = r.gpu.pixels()?;
    assert_eq!((a.draws, a.instances), (1, 1));
    assert_eq!((b.draws, b.instances), (2, 3));
    assert_eq!(b.pipelines, 1, "one column, one pipeline");
    assert_ne!(first, both);
    Ok(())
}

#[test]
fn draws_do_not_share_a_uniform_slot() -> Result<()> {
    let mut r = rig()?;
    seed(&mut r, &[-0.5, 0.5])?;
    // Two cameras in the stack region: fov 0.0 at 0x100, fov 1.0 at 0x200
    // (`fov` follows eye and focus, at byte 28).
    r.vm.write(0x200 + 28, &1.0f32.to_le_bytes())?;
    // Both rows with fov 0, then row 0 again with fov 1: the left triangle is
    // green and the right one is not — unless the second uniform overwrote the first.
    let all = draw(&r, 2, 0x100, 64);
    let left = draw(&r, 1, 0x200, 64);
    replay(&mut r, &frame(&[all, left]))?;
    let px = r.gpu.pixels()?;
    let green = |x: usize, y: usize| px[(y * 64 + x) * 4 + 1];
    assert_eq!(green(16, 33), 255, "left triangle drawn with the second uniform");
    assert_eq!(green(48, 33), 0, "right triangle drawn with the first uniform");
    Ok(())
}
