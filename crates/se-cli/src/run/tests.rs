//! Proves `advance_steps` — and so the `Clock` sequence `Vm::frame` sees — is
//! unaffected by whatever `should_skip_render` decides. Render pacing and
//! simulation pacing are two independent reads of the same `dt`; this test
//! is what stops a future "optimization" from coupling them.

use super::*;

/// Drives `advance_steps` over a fixed `dt` sequence and records the
/// `Clock` each simulated step would see, exactly as `run`'s loop does: a
/// frame index that only ever increments once per step taken. `last_costs`
/// is read only to call `should_skip_render`, whose result is discarded —
/// mirroring that nothing feeds it back into `acc` or the frame index.
fn simulate(dts: &[f32], last_costs: &[Duration]) -> Vec<se_vm::clock::Clock> {
    let mut acc = 0f32;
    let mut index = 0u32;
    let mut clocks = Vec::new();
    for (&dt, &last_cost) in dts.iter().zip(last_costs) {
        let (steps, new_acc) = advance_steps(acc, dt);
        acc = new_acc;
        let _ = should_skip_render(Some(TARGET), last_cost);
        for _ in 0..steps {
            clocks.push(se_vm::clock::Clock::at(index));
            index += 1;
        }
    }
    clocks
}

#[test]
fn render_skip_does_not_change_the_simulation_clock_sequence() {
    let dts = [0.016, 0.033, 0.05, 0.016, 0.1, 0.016, 0.0, 0.2, 0.016];

    let never_over_budget = vec![Duration::ZERO; dts.len()];
    let baseline = simulate(&dts, &never_over_budget);

    // Forced high enough on alternating iterations to trip should_skip_render
    // every other step — never zero steps taken in this run, so the forcing
    // below is exercised, not vacuous.
    let sometimes_over_budget: Vec<Duration> = (0..dts.len())
        .map(|i| if i % 2 == 0 { TARGET + Duration::from_secs(1) } else { Duration::ZERO })
        .collect();
    let with_skips = simulate(&dts, &sometimes_over_budget);

    assert_eq!(baseline, with_skips, "frame indices, t and dt must be identical with or without a skipped render");

    // Sanity: the forced costs actually do and don't trip the decision, so
    // the comparison above isn't accidentally comparing two identical runs.
    assert!(should_skip_render(Some(TARGET), TARGET + Duration::from_secs(1)));
    assert!(!should_skip_render(Some(TARGET), Duration::ZERO));
}

#[test]
fn advance_steps_caps_at_four_and_keeps_the_remainder() {
    // dt is clamped to 0.25s before stepping, so a 10s stall still only
    // drains 4 * STEP from it; the rest is retained, not discarded.
    let (steps, acc) = advance_steps(0.0, 10.0);
    assert_eq!(steps, 4);
    assert!((acc - (0.25 - 4.0 * se_vm::clock::STEP)).abs() < 1e-6);
}

#[test]
fn advance_steps_carries_a_partial_step_to_the_next_call() {
    let (steps, acc) = advance_steps(0.0, se_vm::clock::STEP * 1.5);
    assert_eq!(steps, 1);
    let (steps2, _) = advance_steps(acc, 0.0);
    assert_eq!(steps2, 0, "half a step held over should not round up on its own");
}

/// `handle_render_timeout`: a GPU timeout rolls the render slot back to the
/// module that last produced a picture. The worker thread has already
/// classified the replay failure before this runs, so no real GPU timeout is
/// needed to drive it, only a real `Vm` with two loaded render modules to
/// prove `Vm::load` actually ran.
mod replay_err {
    use super::*;
    use crate::live::Live;
    use crate::render_worker::RenderWorker;
    use se_vm::memory::STACK_SIZE;
    use se_vm::Vm;
    use std::path::{Path, PathBuf};
    use std::process::Command as Proc;

    fn has_wasm_target() -> bool {
        let Ok(o) =
            Proc::new("rustc").args(["--print", "target-libdir", "--target", "wasm32-unknown-unknown"]).output()
        else {
            return false;
        };
        o.status.success() && Path::new(String::from_utf8_lossy(&o.stdout).trim()).is_dir()
    }

    const PRELUDE: &str = concat!(
        "#![no_std]\n",
        "#[panic_handler]\n",
        "fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }\n",
        "include!(\"contract.rs\");\n",
        "#[used]\n#[link_section = \"se.contract-id\"]\npub static SE_CONTRACT_ID: [u8; 8] = CONTRACT_ID.to_le_bytes();\n",
    );

    const RECORD_ONLY: &str = r#"
#[no_mangle]
pub extern "C" fn record() -> u32 {
    unsafe {
        let f = &mut *(arena::FRAME as *mut Frame);
        f.commands = arena::COMMANDS;
        f.count = 0;
        f.present = 0;
        f.epoch = f.epoch.wrapping_add(1);
        0
    }
}
"#;

    const SHADER: &str = r#"@group(0) @binding(0) var<uniform> cam: Camera;
@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: TransformInstance) -> @builtin(position) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
@fragment
fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(cam.fov, 0.0, 0.0, 1.0); }
"#;

    /// One directory and crate name per module: both render modules share a
    /// `--global-base` and would otherwise collide under rustc's defaults.
    fn build_render(dir: &Path, name: &str) -> anyhow::Result<Vec<u8>> {
        let file = dir.join(format!("{name}.rs"));
        std::fs::write(&file, format!("{PRELUDE}{RECORD_ONLY}"))?;
        let out = dir.join(format!("{name}.wasm"));
        let o = Proc::new("rustc")
            .args(["--edition", "2021", "--target", "wasm32-unknown-unknown"])
            .args(["--crate-type", "cdylib", "--crate-name", name])
            .arg("-C").arg("link-arg=--import-memory")
            .arg("-C").arg(format!("link-arg=--global-base={}", Slot::Render.global_base()))
            .arg("-C").arg(format!("link-arg=-zstack-size={STACK_SIZE}"))
            .arg("-C").arg("opt-level=1")
            .arg("-C").arg("panic=abort")
            .arg("-o").arg(&out)
            .arg(&file)
            .output()?;
        anyhow::ensure!(o.status.success(), "building {name}:\n{}", String::from_utf8_lossy(&o.stderr));
        Ok(std::fs::read(out)?)
    }

    /// A `Live` with `render_good` as the cached last-good module and
    /// `render_bad` as the one currently in the `Render` slot — the state a
    /// GPU timeout would find mid-game.
    fn rig(tag: &str) -> anyhow::Result<Live> {
        let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
        let g = se_contract::generate(&wit)?;
        let dir = std::env::temp_dir().join(format!("se-cli-replay-err-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("contract.rs"), &g.rust)?;

        let mut vm = Vm::new(&g.module, 16)?;
        let good = build_render(&dir, "render_good")?;
        vm.load(Slot::Render, "render_good", good.clone())?;
        let bad = build_render(&dir, "render_bad")?;
        vm.load(Slot::Render, "render_bad", bad)?;

        let gpu = se_gpu::Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), 4, 4)?;
        let adapter = gpu.adapter().to_string();
        Ok(Live {
            vm,
            render: RenderWorker::spawn(gpu),
            adapter,
            last_good_render: Some(("render_good".to_string(), good)),
        })
    }

    fn render_name(live: &Live) -> Option<&str> {
        live.vm.module(Slot::Render).map(|m| m.name.as_str())
    }

    #[test]
    fn a_timeout_reloads_the_last_good_render_module_and_notes_rollback() -> anyhow::Result<()> {
        if !has_wasm_target() {
            eprintln!("skipped: wasm32-unknown-unknown is not installed");
            return Ok(());
        }
        let mut live = rig("timeout")?;
        assert_eq!(render_name(&live), Some("render_bad"));

        let mut ide = se_tui::Ide::default();
        let before = ide.log.len();
        handle_render_timeout(&mut live, &mut ide);

        assert_eq!(
            render_name(&live),
            Some("render_good"),
            "a GPU timeout reloads the module that last produced a picture"
        );
        let notes: Vec<&str> = ide.log.iter().skip(before).map(|l| l.text.as_str()).collect();
        assert!(
            notes.contains(&"rolled back render: the previous module is running"),
            "{notes:?}"
        );
        Ok(())
    }
}

/// Proves the decoupling task-245 built actually holds against a real render
/// thread, not only against the pure-function model
/// `render_skip_does_not_change_the_simulation_clock_sequence` exercises
/// above: the sequence of `Clock` values driven into `Vm::frame` must be
/// identical whether the render worker replies to a snapshot instantly or
/// only after an injected delay. `try_send_snapshot` and `try_recv_reply` are
/// both non-blocking at the same call site `run()`'s loop uses them from, and
/// neither one ever feeds back into `acc`, `index`, or the value handed to
/// `vm.frame` — this test drives that through a real thread instead of only
/// reasoning about it.
mod threaded_render {
    use super::*;
    use crate::render_worker::RenderWorker;
    use se_gpu::command::{Command, BEGIN_PASS, DRAW_INSTANCED, END_PASS};
    use se_gpu::Gpu;
    use se_vm::Vm;
    use std::path::PathBuf;

    /// `TransformInstance` and `Camera` come from the prepended `contract.wgsl`,
    /// same fixture `render_worker/tests.rs` uses.
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

    /// A fresh `Vm` with no game/process/render module loaded — `tick`,
    /// stages, and `record` are then all no-ops per `Vm::call`'s "an empty
    /// slot is not an error" — a real headless `Gpu`, and a fixed, always-
    /// valid command list standing in for what `record()` would write. This
    /// test drives `Clock` timing through a real render thread; it does not
    /// need the command list to come from an actual simulated draw.
    fn rig() -> anyhow::Result<(Vm, Gpu, Vec<Command>)> {
        let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
        let g = se_contract::generate(&wit)?;
        let column = g.contract.components().iter().position(|r| r.name == "transform").unwrap() as u32;
        let vm = Vm::new(&g.module, 8)?;
        let gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), 16, 16)?;
        let draw = Command { column, count: 1, ..cmd(DRAW_INSTANCED) };
        let cmds = vec![cmd(BEGIN_PASS), draw, cmd(END_PASS)];
        Ok((vm, gpu, cmds))
    }

    /// Drives `dts` through the same shape as `run()`'s loop body: for each
    /// `dt`, `advance_steps` decides how many fixed sim steps to take; each
    /// one calls `vm.frame(index)`, recorded here as the `Clock` it implies,
    /// then — independent of that, exactly as `run()`'s render call site does
    /// — offers `worker` the current snapshot and polls its last reply.
    fn simulate(vm: &mut Vm, cmds: &[Command], worker: &RenderWorker, dts: &[f32]) -> Vec<se_vm::clock::Clock> {
        let mut acc = 0f32;
        let mut index = 0u32;
        let mut clocks = Vec::new();
        for &dt in dts {
            let (steps, new_acc) = advance_steps(acc, dt);
            acc = new_acc;
            for _ in 0..steps {
                vm.frame(index).expect("frame");
                clocks.push(se_vm::clock::Clock::at(index));
                index += 1;
            }
            worker.try_send_snapshot(vm.meta.clone(), vm.world.clone(), vm.bytes().to_vec(), cmds.to_vec());
            let _ = worker.try_recv_reply();
        }
        clocks
    }

    #[test]
    fn render_thread_lag_does_not_change_the_simulation_clock_sequence() -> anyhow::Result<()> {
        let dts = [0.016, 0.033, 0.05, 0.016, 0.1, 0.016, 0.0, 0.2, 0.016];

        let (mut vm, gpu, cmds) = rig()?;
        let worker = RenderWorker::spawn(gpu);
        let baseline = simulate(&mut vm, &cmds, &worker, &dts);

        // Representative of a slow-but-healthy render: comfortably longer
        // than one loop iteration here (so the bound-1 channel is sometimes
        // still full on the next send, exercising the real drop-and-skip
        // path) yet well under story-241's two-second GPU_BUDGET.
        let delay = Duration::from_millis(80);
        let (mut vm2, gpu2, cmds2) = rig()?;
        let worker2 = RenderWorker::spawn_with_delay(gpu2, delay);
        let with_lag = simulate(&mut vm2, &cmds2, &worker2, &dts);

        assert_eq!(
            baseline, with_lag,
            "Clock sequence into Vm::frame must not depend on render thread timing"
        );
        Ok(())
    }
}
