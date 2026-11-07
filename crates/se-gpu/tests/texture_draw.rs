//! A draw that names a texture samples it; the four texels of a 2x2 image
//! land in the four quadrants of the target.

use anyhow::Result;
use se_gpu::command::{Command, BEGIN_PASS, DRAW_FULLSCREEN, END_PASS, NONE};
use se_gpu::resources::decode;
use se_gpu::Gpu;
use se_vm::Vm;
use std::path::PathBuf;

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

#[test]
fn a_texture_draw_samples_the_image() -> Result<()> {
    let colours: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]];
    let img = image::RgbaImage::from_raw(2, 2, colours.concat()).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)?;

    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let vm = Vm::new(&g.module, 128)?;
    let mut gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), 64, 64)?;
    gpu.set_resource(3, decode("png", png.get_ref())?.unwrap());
    let draw = Command { kind: DRAW_FULLSCREEN, texture: 3, ..cmd(0) };
    gpu.replay(&vm.meta, &vm.world, vm.bytes(), &[cmd(BEGIN_PASS), draw, cmd(END_PASS)])?;
    let px = gpu.pixels()?;
    let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
    // The sampler is linear and a pixel centre sits half a pixel off a texel
    // centre, so a few levels of the neighbour bleed in.
    let near = |got: &[u8], want: [u8; 4]| got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 16);
    for (x, y, c) in [(16, 16, 0), (48, 16, 1), (16, 48, 2), (48, 48, 3)] {
        assert!(near(at(x, y), colours[c]), "({x},{y}) is {:?}, want {:?}", at(x, y), colours[c]);
    }

    // A texture id naming a mesh slot's absence is refused.
    let bad = Command { kind: DRAW_FULLSCREEN, texture: 9, ..cmd(0) };
    let e = gpu.replay(&vm.meta, &vm.world, vm.bytes(), &[cmd(BEGIN_PASS), bad, cmd(END_PASS)]).unwrap_err();
    assert!(format!("{e:#}").contains("asset 9 as a texture"), "{e:#}");
    Ok(())
}

fn rig(w: u32, h: u32) -> Result<(Vm, Gpu)> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let vm = Vm::new(&g.module, 128)?;
    let gpu = Gpu::new(&vm.meta, &format!("{}\n{SHADER}", g.wgsl), w, h)?;
    Ok((vm, gpu))
}

#[test]
fn a_fullscreen_draw_samples_an_earlier_pass_target() -> Result<()> {
    let (vm, mut gpu) = rig(64, 64)?;

    let pass1 = Command { target: 1, clear: [0.0, 1.0, 0.0, 1.0], ..cmd(BEGIN_PASS) };
    let pass2 = Command { target: 0, clear: [0.0, 0.0, 0.0, 1.0], ..cmd(BEGIN_PASS) };
    let composite = Command { kind: DRAW_FULLSCREEN, sample: 1, ..cmd(0) };
    gpu.replay(
        &vm.meta,
        &vm.world,
        vm.bytes(),
        &[pass1, cmd(END_PASS), pass2, composite, cmd(END_PASS)],
    )?;
    let px = gpu.pixels()?;
    assert_eq!(
        &px[0..4],
        &[0, 255, 0, 255],
        "present must show pass 1's offscreen target, not the pass 2 clear or white"
    );
    Ok(())
}

#[test]
fn sample_naming_a_target_no_earlier_pass_wrote_is_refused() -> Result<()> {
    let (vm, mut gpu) = rig(64, 64)?;
    let draw = Command { kind: DRAW_FULLSCREEN, sample: 5, ..cmd(0) };
    let e = gpu
        .replay(&vm.meta, &vm.world, vm.bytes(), &[cmd(BEGIN_PASS), draw, cmd(END_PASS)])
        .unwrap_err();
    assert!(format!("{e:#}").contains("no earlier pass"), "{e:#}");
    Ok(())
}

#[test]
fn a_draw_cannot_set_both_texture_and_sample() -> Result<()> {
    let (vm, mut gpu) = rig(64, 64)?;
    let colours: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]];
    let img = image::RgbaImage::from_raw(2, 2, colours.concat()).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)?;
    gpu.set_resource(3, decode("png", png.get_ref())?.unwrap());

    let bad = Command { kind: DRAW_FULLSCREEN, texture: 3, sample: 1, ..cmd(0) };
    let e = gpu
        .replay(&vm.meta, &vm.world, vm.bytes(), &[cmd(BEGIN_PASS), bad, cmd(END_PASS)])
        .unwrap_err();
    assert!(format!("{e:#}").contains("both `texture` and `sample`"), "{e:#}");
    Ok(())
}

/// `render::BLANK` (generated by `se-contract`'s `emit_render`) leaves
/// `sample` at its `Command::ZERO` default, 0, rather than `NONE` — mirrored
/// here since `BLANK` itself lives in generated, per-contract code. A draw
/// built from it with only `texture` set must sample that texture, not be
/// misread as setting both `texture` and `sample`.
#[test]
fn a_blank_derived_draw_with_only_texture_set_samples_the_image() -> Result<()> {
    let colours: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]];
    let img = image::RgbaImage::from_raw(2, 2, colours.concat()).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)?;

    let (vm, mut gpu) = rig(64, 64)?;
    gpu.set_resource(3, decode("png", png.get_ref())?.unwrap());

    let draw = Command { kind: DRAW_FULLSCREEN, texture: 3, sample: 0, ..cmd(0) };
    gpu.replay(&vm.meta, &vm.world, vm.bytes(), &[cmd(BEGIN_PASS), draw, cmd(END_PASS)])?;
    let px = gpu.pixels()?;
    let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
    let near = |got: &[u8], want: [u8; 4]| got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 16);
    assert!(near(at(16, 16), colours[0]), "({},{}) is {:?}, want {:?}", 16, 16, at(16, 16), colours[0]);
    Ok(())
}
