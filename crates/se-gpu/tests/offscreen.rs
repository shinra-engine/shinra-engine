//! The full story-45 round trip in one frame: an offscreen pass draws real
//! per-pixel content into target 1, a present pass `DRAW_FULLSCREEN`s with
//! `sample=1`, and the final readback is checked against the offscreen
//! pass's known content — the whole target-write-then-sample path, not its
//! write half (texture_draw.rs) and its sample half (also texture_draw.rs)
//! checked separately.

use anyhow::Result;
use se_gpu::command::{Command, BEGIN_PASS, DRAW_FULLSCREEN, END_PASS, NONE};
use se_gpu::resources::decode;
use se_gpu::Gpu;
use se_vm::Vm;
use std::path::PathBuf;

/// Same fullscreen-triangle vertex shader as texture_draw.rs: no instance
/// input, so the one pipeline built for `column == NONE` serves every
/// `DRAW_FULLSCREEN` in this frame, whether it samples a decoded texture or
/// an earlier pass's target — `bind_for` picks which view binding 1 gets.
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

#[test]
fn an_offscreen_pass_draws_and_a_present_pass_composites_it_in_one_frame() -> Result<()> {
    let colours: [[u8; 4]; 4] = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
    ];
    let img = image::RgbaImage::from_raw(2, 2, colours.concat()).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)?;

    let (vm, mut gpu) = rig(64, 64)?;
    gpu.set_resource(3, decode("png", png.get_ref())?.unwrap());

    // Pass 1: an offscreen pass (target=1) draws the quadrant image — real,
    // spatially varying content, not a flat clear.
    let pass1 = Command {
        target: 1,
        clear: [0.0, 0.0, 0.0, 1.0],
        ..cmd(BEGIN_PASS)
    };
    let paint = Command {
        kind: DRAW_FULLSCREEN,
        texture: 3,
        ..cmd(0)
    };
    // Pass 2: the present pass (target=0) never draws its own content; it
    // only composites pass 1's target via `sample`, so every present pixel
    // must trace back to what pass 1 drew.
    let pass2 = Command {
        target: 0,
        clear: [1.0, 1.0, 1.0, 1.0],
        ..cmd(BEGIN_PASS)
    };
    let composite = Command {
        kind: DRAW_FULLSCREEN,
        sample: 1,
        ..cmd(0)
    };

    let report = gpu.replay(
        &vm.meta,
        &vm.world,
        vm.bytes(),
        &[pass1, paint, cmd(END_PASS), pass2, composite, cmd(END_PASS)],
    )?;
    assert_eq!(
        report.passes, 2,
        "the offscreen draw and the composite are separate passes"
    );

    let px = gpu.pixels()?;
    let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
    let near = |got: &[u8], want: [u8; 4]| got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 16);
    for (x, y, c) in [(16, 16, 0), (48, 16, 1), (16, 48, 2), (48, 48, 3)] {
        assert!(
            near(at(x, y), colours[c]),
            "present ({x},{y}) is {:?}, want pass 1's quadrant {:?} (round trip through target 1)",
            at(x, y),
            colours[c]
        );
    }
    Ok(())
}
