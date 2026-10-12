//! A draw that asks for more than the GPU buffers or the pool hold is an
//! error naming the limit, never a wgpu validation panic or a silent read of
//! the next column.

use anyhow::Result;
use se_gpu::command::{Command, BEGIN_PASS, DRAW_INSTANCED, END_PASS};
use se_gpu::Gpu;
use se_vm::Vm;
use std::path::PathBuf;

const SHADER: &str = r#"@group(0) @binding(0) var<uniform> cam: Camera;
@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: TransformInstance) -> @builtin(position) vec4<f32> {
    return vec4<f32>(inst.pos.xy, 0.0, 1.0);
}
@fragment
fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
"#;

fn cmd(kind: u32, column: u32, count: u32, uniform_len: u32) -> Command {
    Command {
        kind,
        vertices: 3,
        shader: 0,
        column,
        count,
        uniform_off: 0,
        uniform_len,
        clear: [0.0, 0.0, 0.0, 1.0],
    }
}

/// Replay one instanced draw of `count` rows against a world of `rows` rows.
fn replay(rows: u32, count: u32, uniform_len: u32) -> Result<()> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let transform =
        g.contract.components().iter().position(|r| r.name == "transform").unwrap() as u32;
    let vm = Vm::new(&g.module, rows)?;
    let gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), transform, 32, 32)?;
    let cmds = [
        cmd(BEGIN_PASS, 0, 0, 0),
        cmd(DRAW_INSTANCED, transform, count, uniform_len),
        cmd(END_PASS, 0, 0, 0),
    ];
    gpu.replay(&vm.meta, &vm.world, vm.bytes(), &cmds)?;
    Ok(())
}

fn refused(r: Result<()>, needle: &str) {
    let e = r.expect_err("the draw should be refused").to_string();
    assert!(e.contains(needle), "`{e}` does not mention `{needle}`");
}

#[test]
fn more_instances_than_the_buffer_holds_is_refused() {
    refused(replay(4096, 4097, 0), "4096");
}

#[test]
fn more_rows_than_the_pool_holds_is_refused() {
    refused(replay(1024, 2000, 0), "pool holds only 1024");
}

#[test]
fn an_oversized_uniform_is_refused() {
    refused(replay(128, 1, 257), "256");
}

#[test]
fn a_draw_at_the_limit_is_accepted() -> Result<()> {
    replay(4096, 4096, 0)
}
