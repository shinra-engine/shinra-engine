//! `Gpu::pixels`'s wall-clock bound.
//!
//! A healthy replay finishes long before `GPU_BUDGET` and is unaffected by
//! it. `pixels_with_budget` is the same call parameterized on the budget — a
//! seam reserved for this test: a budget of zero cannot see even a tiny,
//! just-submitted frame as already complete, so it deterministically forces
//! `wgpu::PollError::Timeout` without waiting out the real 2-second value or
//! fabricating an artificially slow draw.

use anyhow::Result;
use se_gpu::command::{Command, BEGIN_PASS, DRAW_INSTANCED, END_PASS};
use se_gpu::{Gpu, GPU_BUDGET};
use se_vm::Vm;
use std::path::PathBuf;
use std::time::Duration;
use wgpu::PollError;

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
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let column = g
        .contract
        .components()
        .iter()
        .position(|r| r.name == "transform")
        .unwrap() as u32;
    let vm = Vm::new(&g.module, 128)?;
    let gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), 64, 64)?;
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
        target: 0,
        sample: 0,
        render_flags: 0,
        clear: [0.2, 0.4, 0.6, 1.0],
    }
}

fn a_frame(r: &mut Rig) -> Result<()> {
    let draw = Command { column: r.column, count: 1, ..cmd(DRAW_INSTANCED) };
    r.gpu
        .replay(&r.vm.meta, &r.vm.world, r.vm.bytes(), &[cmd(BEGIN_PASS), draw, cmd(END_PASS)])
        .map(|_| ())
}

#[test]
fn a_fast_replay_is_unaffected_by_the_budget() -> Result<()> {
    let mut r = rig()?;
    a_frame(&mut r)?;
    let px = r.gpu.pixels()?;
    assert_eq!(px.len(), 64 * 64 * 4);
    Ok(())
}

#[test]
fn a_zero_budget_times_out() -> Result<()> {
    let mut r = rig()?;
    a_frame(&mut r)?;
    let err = r
        .gpu
        .pixels_with_budget(Duration::ZERO)
        .expect_err("a zero-length wait cannot see a just-submitted frame as already complete");
    assert!(
        matches!(err.downcast_ref::<PollError>(), Some(PollError::Timeout)),
        "`{err:#}` should be a PollError::Timeout"
    );
    assert!(
        format!("{err:#}").contains("GPU replay exceeded its"),
        "`{err:#}` should name the budget it exceeded"
    );
    Ok(())
}

#[test]
fn the_real_budget_is_two_seconds() {
    // Pinned so a future change to GPU_BUDGET is a deliberate edit here too,
    // not a silent drift in the one bound this whole story is about.
    assert_eq!(GPU_BUDGET, Duration::from_secs(2));
}
