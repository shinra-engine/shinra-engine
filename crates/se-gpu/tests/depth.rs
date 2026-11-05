//! `BEGIN_PASS.render_flags` bit 0 (`FLAG_DEPTH`): depth, not row or draw
//! order, decides which of two overlapping instanced rows wins, and a
//! `DRAW_FULLSCREEN` inside the same kind of pass ignores depth entirely.
//! See `pipeline.rs::build` and `replay.rs::replay_inner`.

use anyhow::Result;
use se_gpu::command::{
    Command, BEGIN_PASS, DRAW_FULLSCREEN, DRAW_INSTANCED, END_PASS, FLAG_DEPTH, NONE,
};
use se_gpu::Gpu;
use se_vm::world::COLUMN_BYTES;
use se_vm::Vm;
use std::path::PathBuf;

/// Each instance's `pos` doubles as its NDC depth (`pos.z`, via a clip-space
/// triangle covering the whole viewport) and its tint (`pos.xy`), so the
/// final pixels say which row's fragment survived.
const INSTANCED_SHADER: &str = r#"
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) tint: vec3<f32> };
@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: TransformInstance) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: VsOut;
    o.clip = vec4<f32>(corners[vi], inst.pos.z, 1.0);
    o.tint = inst.pos;
    return o;
}
@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> { return vec4<f32>(i.tint, 1.0); }
"#;

/// A `DRAW_FULLSCREEN` has no instance input, so its per-draw depth and
/// color come from the uniform slot instead — a small struct of our own in
/// place of the usual `Camera`, since binding 0's type is up to the module.
const FULLSCREEN_SHADER: &str = r#"
struct Slot { z: f32, r: f32, g: f32, b: f32 };
@group(0) @binding(0) var<uniform> slot: Slot;
struct VsOut { @builtin(position) clip: vec4<f32> };
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: VsOut;
    o.clip = vec4<f32>(corners[vi], slot.z, 1.0);
    return o;
}
@fragment
fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(slot.r, slot.g, slot.b, 1.0); }
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

fn near(got: &[u8], want: [u8; 4]) -> bool {
    got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 16)
}

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
    let gpu = Gpu::new(&vm.meta, &format!("{}\n{INSTANCED_SHADER}", g.wgsl), 16, 16)?;
    Ok(Rig { vm, gpu, column })
}

/// Writes `transform.pos` for rows 0 and 1; every other field stays zero.
fn seed_pos(r: &mut Rig, rows: [[f32; 3]; 2]) -> Result<()> {
    let col = (r.vm.meta.columns_at + r.column * COLUMN_BYTES) as usize;
    let u = |at: usize| u32::from_le_bytes(r.vm.bytes()[at..at + 4].try_into().unwrap());
    let (base, stride) = (u(col), u(col + 8));
    for (i, pos) in rows.iter().enumerate() {
        let mut row = vec![0u8; stride as usize];
        row[0..4].copy_from_slice(&pos[0].to_le_bytes());
        row[4..8].copy_from_slice(&pos[1].to_le_bytes());
        row[8..12].copy_from_slice(&pos[2].to_le_bytes());
        r.vm.write(base + i as u32 * stride, &row)?;
    }
    Ok(())
}

#[test]
fn depth_decides_which_overlapping_instance_wins_not_row_order() -> Result<()> {
    // Row tint doubles as (r, g, _); z is the third component. Nearer (lower
    // z) must win under compare `Less` against the 1.0 depth clear.
    let near_row = [1.0, 0.0, 0.1]; // red, z=0.1
    let far_row = [0.0, 1.0, 0.9]; // green, z=0.9

    for rows in [[near_row, far_row], [far_row, near_row]] {
        let mut r = rig()?;
        seed_pos(&mut r, rows)?;
        let begin = Command { render_flags: FLAG_DEPTH, ..cmd(BEGIN_PASS) };
        let draw = Command {
            kind: DRAW_INSTANCED,
            column: r.column,
            count: 2,
            vertices: 3,
            ..cmd(0)
        };
        r.gpu.replay(&r.vm.meta, &r.vm.world, r.vm.bytes(), &[begin, draw, cmd(END_PASS)])?;
        let px = r.gpu.pixels()?;
        for p in px.chunks(4) {
            assert!(
                near(p, [255, 0, 26, 255]),
                "the nearer (red) row must win regardless of row order, got {p:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn fullscreen_inside_depth_pass_ignores_existing_depth() -> Result<()> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let mut vm = Vm::new(&g.module, 128)?;
    let mut gpu = Gpu::new(&vm.meta, &format!("{}\n{FULLSCREEN_SHADER}", g.wgsl), 16, 16)?;

    // Two separate arena slots: each draw's uniform bytes are read from `mem`
    // as it stands when `replay()` is called, so the two draws cannot share
    // one address and still carry different values.
    let heap = vm.meta.heap;
    let slot = |z: f32, rgb: [f32; 3]| -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&z.to_le_bytes());
        b[4..8].copy_from_slice(&rgb[0].to_le_bytes());
        b[8..12].copy_from_slice(&rgb[1].to_le_bytes());
        b[12..16].copy_from_slice(&rgb[2].to_le_bytes());
        b
    };
    // Drawn first, nearer (z=0.1), red. Under a correct fullscreen pipeline
    // this writes no depth at all; under a wrongly depth-tested one it would
    // write 0.1 and block the farther draw below.
    vm.write(heap, &slot(0.1, [1.0, 0.0, 0.0]))?;
    // Drawn second, farther (z=0.9), blue. A `Less` compare against a
    // written 0.1 would fail this fragment, leaving red on screen instead.
    vm.write(heap + 16, &slot(0.9, [0.0, 0.0, 1.0]))?;

    let begin = Command { render_flags: FLAG_DEPTH, ..cmd(BEGIN_PASS) };
    let draw1 = Command { kind: DRAW_FULLSCREEN, uniform_off: heap, uniform_len: 16, ..cmd(0) };
    let draw2 =
        Command { kind: DRAW_FULLSCREEN, uniform_off: heap + 16, uniform_len: 16, ..cmd(0) };
    gpu.replay(&vm.meta, &vm.world, vm.bytes(), &[begin, draw1, draw2, cmd(END_PASS)])?;
    let px = gpu.pixels()?;
    for p in px.chunks(4) {
        assert!(
            near(p, [0, 0, 255, 255]),
            "the later (farther) fullscreen draw must still win, got {p:?}"
        );
    }
    Ok(())
}
