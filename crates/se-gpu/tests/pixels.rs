//! The whole engine, from `contract.wit` to pixels.
//!
//! contract.wit  ─┬─→ contract.rs   the modules compile against it
//!                ├─→ contract.wgsl the shader's instance input *is* the record
//!                └─→ contract.wasm the host reads it, compiling no game in
//!
//! game.wasm writes control vars, process.wasm writes a column, render.wasm
//! records a command list, and the host replays it — uploading that column
//! with no repacking, because both sides got their offsets from one file.

use anyhow::{bail, Result};
use se_gpu::{command, Gpu};
use se_vm::memory::{Slot, STACK_SIZE};
use se_vm::Vm;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;

#[path = "../../se-vm/tests/common/wasm_target.rs"]
mod wasm_target;

const W: u32 = 192;
const H: u32 = 192;

fn build(dir: &Path, name: &str, slot: Slot, src: &str) -> Result<Vec<u8>> {
    let file = dir.join(format!("{name}.rs"));
    std::fs::write(&file, src)?;
    let out = dir.join(format!("{name}.wasm"));
    let o = Proc::new("rustc")
        .args(["--edition", "2021", "--target", "wasm32-unknown-unknown"])
        .args(["--crate-type", "cdylib", "--crate-name", name])
        .arg("-C").arg("link-arg=--import-memory")
        .arg("-C").arg(format!("link-arg=--global-base={}", slot.global_base()))
        .arg("-C").arg(format!("link-arg=-zstack-size={STACK_SIZE}"))
        .arg("-C").arg("opt-level=1")
        .arg("-C").arg("panic=abort")
        .arg("-o").arg(&out)
        .arg(&file)
        .output()?;
    if !o.status.success() {
        bail!("building {name}:\n{}", String::from_utf8_lossy(&o.stderr));
    }
    Ok(std::fs::read(out)?)
}

const PRELUDE: &str = concat!(
    "#![no_std]\n",
    "#[panic_handler]\n",
    "fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }\n",
    "include!(\"contract.rs\");\n",
    "#[used]\n#[link_section = \"se.contract-id\"]\npub static SE_CONTRACT_ID: [u8; 8] = CONTRACT_ID.to_le_bytes();\n",
    "#[inline]\nunsafe fn col(i: u32) -> Column { *((arena::COLUMNS + i * 16) as *const Column) }\n",
    "#[inline]\nunsafe fn pool(i: u32) -> Pool { *((arena::POOLS + i * 8) as *const Pool) }\n",
    "#[inline]\nunsafe fn row<T>(c: &Column, i: u32) -> *mut T { (c.base + i * c.stride) as *mut T }\n",
);

const GAME: &str = r#"
#[no_mangle]
pub extern "C" fn tick(n: u32) -> u32 {
    unsafe {
        let look = &mut *(arena::LOOK as *mut Look);
        look.bodies = 3 + n % 5;
        look.graph = n % 2;
        let cam = &mut *(arena::CAMERA as *mut Camera);
        cam.fov = 0.8;
        cam.yaw = n as f32 * 0.1;
        0
    }
}
"#;

const PROCESS: &str = r#"
#[no_mangle]
pub extern "C" fn se_stage_run__place() -> u32 {
    unsafe {
        let n = (&*(arena::LOOK as *const Look)).bodies.min(pool(arena::POOL_BODIES).cap);
        let (co, ct) = (col(arena::COL_ORBIT), col(arena::COL_TRANSFORM));
        let mut i = 0u32;
        while i < n {
            let o = &*row::<Orbit>(&co, i);
            let t = &mut *row::<Transform>(&ct, i);
            t.pos = [o.center[0], o.center[1], 0.0];
            t.rot = [0.0, 0.0, 0.0, 1.0];
            t.scale = [o.radius, o.radius, o.radius];
            i += 1;
        }
        n
    }
}
"#;

/// `TransformInstance` and `Camera` are not declared here: the host prepends
/// `contract.wgsl`, so the instance input is the component record itself.
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

const RENDER: &str = r#"
#[no_mangle]
pub extern "C" fn record() -> u32 {
    unsafe {
        let look = &*(arena::LOOK as *const Look);
        let n = look.bodies.min(pool(arena::POOL_BODIES).cap);
        let zero = Command {
            kind: 0, vertices: 0, shader: 0, column: u32::MAX, count: 0,
            uniform_off: 0, uniform_len: 0, mesh: u32::MAX, texture: u32::MAX,
            clear_r: 0.0, clear_g: 0.0, clear_b: 0.0, clear_a: 1.0,
        };
        let cmds = arena::COMMANDS as *mut Command;
        // The look is a control var: swapping game.wasm changes the picture
        // with no shader rebuilt and no asset touched.
        let bg = if look.graph == 0 { 0.05 } else { 0.35 };
        *cmds.add(0) = Command { kind: 0, clear_r: bg, clear_g: bg * 0.5, clear_b: bg, ..zero };
        *cmds.add(1) = Command {
            kind: 1, column: arena::COL_TRANSFORM, count: n,
            uniform_off: arena::CAMERA, uniform_len: Camera::SIZE, ..zero
        };
        *cmds.add(2) = Command { kind: 3, ..zero };
        let f = &mut *(arena::FRAME as *mut Frame);
        f.commands = arena::COMMANDS;
        f.count = 3;
        f.present = 0;
        f.epoch = f.epoch.wrapping_add(1);
        3
    }
}
"#;

fn embed(section: &str, bytes: &[u8]) -> String {
    let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
    format!(
        "#[used]\n#[link_section = \"{section}\"]\npub static S_{}: [u8; {}] = [{}];\n",
        section.replace('.', "_").to_uppercase(),
        bytes.len(),
        list.join(",")
    )
}

fn stage_section() -> Vec<u8> {
    use se_vm::stages::{encode, READ, WRITE};
    encode("place", se_vm::stages::ROW, &[("orbit", READ, 0), ("transform", WRITE, 0)])
}

#[test]
fn contract_wit_to_pixels() -> Result<()> {
    wasm_target::require_wasm_target();
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    // One directory per build: rustc names its intermediate objects after the
    // crate, so two concurrent builds in one directory collide.
    let dir = std::env::temp_dir().join(format!(
        "se-gpu-pixels-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("contract.rs"), &g.rust)?;

    let comps = g.contract.components();
    let orbit = comps.iter().position(|r| r.name == "orbit").unwrap() as u32;

    let game = build(&dir, "game", Slot::Game, &format!("{PRELUDE}{GAME}"))?;
    let process = build(
        &dir,
        "process",
        Slot::Process,
        &format!("{PRELUDE}{}{PROCESS}", embed(se_vm::stages::SECTION, &stage_section())),
    )?;
    let render = build(
        &dir,
        "render",
        Slot::Render,
        &format!("{PRELUDE}{}{RENDER}", embed("se.wgsl", SHADER.as_bytes())),
    )?;

    let mut vm = Vm::new(&g.module, 128)?;
    vm.load(Slot::Game, "game", game)?;
    vm.load(Slot::Process, "process", process)?;
    vm.load(Slot::Render, "render", render.clone())?;

    // The shader rides in the render module's own custom section, read out of
    // the file — no instantiation needed for bytes.
    let wgsl = std::str::from_utf8(
        se_contract::wasm::section(&render, "se.wgsl").expect("se.wgsl"),
    )?
    .to_string();
    let mut gpu = Gpu::new(&vm.meta, &format!("{}\n{wgsl}", g.wgsl), W, H)?;
    eprintln!("adapter: {}", gpu.adapter());

    // Seed the column the host owns.
    for i in 0..8u32 {
        let mut b = Vec::new();
        for f in [
            -0.6 + i as f32 * 0.17,
            -0.4 + (i % 3) as f32 * 0.35,
            0.0,
            0.10 + (i % 4) as f32 * 0.03,
            0.0,
            0.0,
            1.0,
        ] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        b.resize(vm.world.stride[orbit as usize] as usize, 0);
        vm.write(vm.world.row_at(orbit, i), &b)?;
    }

    let mut frames = Vec::new();
    for n in [0u32, 1] {
        let (rows, n_cmds) = vm.frame(n)?;
        let (cmds, epoch) = command::read(&vm.meta, vm.bytes())?;
        assert_eq!(n_cmds, 3);
        assert_eq!(cmds.len(), 3);
        assert_eq!(epoch, n + 1, "render.wasm stamps every frame it records");

        let report = gpu.replay(&vm.meta, &vm.world, vm.bytes(), &cmds)?;
        assert_eq!(report.passes, 1);
        assert_eq!(report.draws, 1);
        assert_eq!(report.instances, rows, "the draw covers exactly the rows the stage wrote");

        let px = gpu.pixels()?;
        let bg = [
            (report.clear[0] * 255.0).round() as u8,
            (report.clear[1] * 255.0).round() as u8,
            (report.clear[2] * 255.0).round() as u8,
        ];
        let drawn = px.chunks(4).filter(|p| [p[0], p[1], p[2]] != bg).count();
        eprintln!("frame {n}: rows {rows}, bg {bg:?}, {drawn} pixels drawn");
        assert!(drawn > 0, "frame {n} drew nothing");
        frames.push((bg, drawn, px));
    }

    // A control var chose the look, and it reached the screen.
    let (bg0, d0, p0) = &frames[0];
    let (bg1, d1, p1) = &frames[1];
    assert_ne!(bg0, bg1, "look.graph changed the clear colour");
    assert_ne!(d0, d1, "look.bodies changed how much was drawn");
    let differing = p0.chunks(4).zip(p1.chunks(4)).filter(|(a, b)| a != b).count();
    assert!(differing > (W * H / 2) as usize, "only {differing} pixels differ");
    Ok(())
}
