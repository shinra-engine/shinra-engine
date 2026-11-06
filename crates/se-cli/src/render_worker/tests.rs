//! `RenderWorker::spawn` against a real headless `Gpu`, built the same
//! lightweight way `crates/se-gpu/tests/timeout.rs` does: the contract alone,
//! no game/process/render `.wasm` compiled, since nothing here calls
//! `Vm::frame` — the command list is hand-built, standing in for task-247's
//! future `run.rs` caller.

use super::*;
use anyhow::Result;
use se_gpu::command::{BEGIN_PASS, DRAW_INSTANCED, END_PASS};
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

struct Snapshot {
    meta: Meta,
    world: World,
    mem: Vec<u8>,
    cmds: Vec<Command>,
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

/// A fresh `Gpu` plus a snapshot of inputs it can replay, independent each
/// call so a baseline and a worker never share one `Gpu`.
fn rig() -> Result<(Gpu, Snapshot)> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let column = g
        .contract
        .components()
        .iter()
        .position(|r| r.name == "transform")
        .unwrap() as u32;
    let vm = se_vm::Vm::new(&g.module, 8)?;
    let gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), 16, 16)?;
    let draw = Command { column, count: 1, ..cmd(DRAW_INSTANCED) };
    let cmds = vec![cmd(BEGIN_PASS), draw, cmd(END_PASS)];
    let snap = Snapshot { meta: vm.meta.clone(), world: vm.world.clone(), mem: vm.bytes().to_vec(), cmds };
    Ok((gpu, snap))
}

fn recv_within(worker: &RenderWorker, budget: Duration) -> RenderReply {
    let deadline = Instant::now() + budget;
    loop {
        if let Some(r) = worker.try_recv_reply() {
            return r;
        }
        assert!(Instant::now() < deadline, "worker never replied within {budget:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn snapshot_round_trips_to_one_reply_matching_a_direct_replay() -> Result<()> {
    // Baseline: the same inputs, replayed directly on a `Gpu` the worker
    // never sees.
    let (mut baseline_gpu, baseline) = rig()?;
    let expected = baseline_gpu
        .replay(&baseline.meta, &baseline.world, &baseline.mem, &baseline.cmds)
        .and_then(|_| baseline_gpu.pixels())?;

    let (gpu, snap) = rig()?;
    let worker = RenderWorker::spawn(gpu);
    assert!(worker.try_send_snapshot(snap.meta, snap.world, snap.mem, snap.cmds));

    match recv_within(&worker, Duration::from_secs(5)) {
        RenderReply::Picture(px) => assert_eq!(px, expected, "worker pixels differ from a direct replay"),
        RenderReply::TimedOut => panic!("worker reported a GPU timeout on a healthy Gpu"),
        RenderReply::Failed(e) => panic!("worker reported a replay error: {e}"),
    }
    Ok(())
}
