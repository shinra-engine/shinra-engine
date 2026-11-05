//! A draw that names a mesh draws the mesh's vertices, not `command.vertices`.

use anyhow::Result;
use se_gpu::command::{Command, BEGIN_PASS, DRAW_INSTANCED, END_PASS};
use se_gpu::resources::decode;
use se_gpu::Gpu;
use se_vm::Vm;
use std::path::PathBuf;

const SHADER: &str = r#"
struct VsOut { @builtin(position) clip: vec4<f32> };
@vertex
fn vs_main(inst: TransformInstance, v: MeshVertex) -> VsOut {
    var o: VsOut;
    o.clip = vec4<f32>(v.pos.xy * 0.5 + inst.pos.xy, 0.0, 1.0);
    return o;
}
@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> { return vec4<f32>(1.0, 1.0, 1.0, 1.0); }
"#;

const QUAD: &str = "v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3 4\n";
const TRI: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";

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
        sample: 0,
        render_flags: 0,
    }
}

fn shoot(obj: &str, mesh: u32) -> Result<Vec<u8>> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let column = g.contract.components().iter().position(|r| r.name == "transform").unwrap() as u32;
    let vm = Vm::new(&g.module, 128)?;
    let mut gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), 32, 32)?;
    gpu.set_resource(0, decode("obj", obj.as_bytes())?.unwrap());
    let draw = Command { kind: DRAW_INSTANCED, column, count: 1, mesh, ..cmd(0) };
    let cmds = [cmd(BEGIN_PASS), draw, cmd(END_PASS)];
    gpu.replay(&vm.meta, &vm.world, vm.bytes(), &cmds)?;
    gpu.pixels()
}

#[test]
fn a_mesh_draw_uses_the_meshs_vertices() -> Result<()> {
    let quad = shoot(QUAD, 0)?;
    let tri = shoot(TRI, 0)?;
    let lit = |p: &[u8]| p.chunks(4).filter(|c| c[0] > 128).count();
    assert!(lit(&quad) > 0 && lit(&quad) < quad.len() / 4, "the quad is neither flat nor empty");
    assert_ne!(quad, tri, "a different mesh draws different pixels");
    assert!(lit(&quad) > lit(&tri));
    Ok(())
}

#[test]
fn a_mesh_id_that_is_not_a_mesh_is_refused() -> Result<()> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let column = g.contract.components().iter().position(|r| r.name == "transform").unwrap() as u32;
    let vm = Vm::new(&g.module, 128)?;
    let mut gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), 32, 32)?;
    let draw = Command { kind: DRAW_INSTANCED, column, count: 1, mesh: 5, ..cmd(0) };
    let cmds = [cmd(BEGIN_PASS), draw, cmd(END_PASS)];
    let e = gpu.replay(&vm.meta, &vm.world, vm.bytes(), &cmds).unwrap_err();
    assert!(format!("{e:#}").contains("asset 5 as a mesh but it is absent"), "{e:#}");
    Ok(())
}
