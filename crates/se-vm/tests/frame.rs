//! One contract, three modules, one memory — built and run here.
//!
//! The modules are compiled by this test, the way `se/tests/wasm_backend.rs`
//! already compiles one: a contract is only proved by something built against
//! it, and a fixture checked in as bytes would stop tracking the generator.

mod common;

use anyhow::Result;
use common::{build, contract_dir, embed, prelude, require_wasm_target};
use se_vm::memory::Slot;
use se_vm::Vm;
use std::path::PathBuf;

const GAME: &str = r#"
/// The whole of a control module: it writes the control var set and nothing
/// else. No spawn, no despawn, no host call — and therefore no import.
#[no_mangle]
pub extern "C" fn tick(n: u32) -> u32 {
    unsafe {
        let look = &mut *(arena::LOOK as *mut Look);
        look.bodies = 2 + n % 4;
        look.asset = n % 2;
        look.graph = n / 2 % 2;
        let cam = &mut *(arena::CAMERA as *mut Camera);
        cam.eye = [0.0, 1.5, 6.0];
        cam.fov = 0.9;
        cam.yaw = n as f32 * 0.25;
        0
    }
}
"#;

/// The `se.stages` section a process module carries, built with the engine's
/// own encoder. Two stages, so the ordering is visible; `motion` reads `orbit`
/// and writes `transform`, `settle` only writes `transform`.
fn stage_section() -> Vec<u8> {
    use se_vm::stages::{encode, READ, WRITE};
    let mut v = encode("motion", &[("orbit", READ), ("transform", WRITE)]);
    v.extend_from_slice(&encode("settle", &[("transform", WRITE)]));
    v
}

const PROCESS: &str = r#"
/// One stage. No query, no entity: row `i` is row `i` in every column of the
/// pool, and how many rows exist is a control var.
#[no_mangle]
pub extern "C" fn se_stage_run__motion() -> u32 {
    unsafe {
        let want = (&*(arena::LOOK as *const Look)).bodies;
        let p = pool(arena::POOL_BODIES);
        let n = want.min(p.cap);
        let (co, ct) = (col(arena::COL_ORBIT), col(arena::COL_TRANSFORM));
        let mut i = 0u32;
        while i < n {
            let o = &*row::<Orbit>(&co, i);
            let t = &mut *row::<Transform>(&ct, i);
            t.pos = [o.center[0] + o.radius, o.center[1] + o.phase, o.center[2] + o.speed];
            t.rot = [0.0, 0.0, 0.0, 1.0];
            t.scale = [o.tilt, o.tilt, o.tilt];
            i += 1;
        }
        n
    }
}

/// A second stage, to show that order is declared rather than inferred from a
/// filename. It only settles the rotation the first one left.
#[no_mangle]
pub extern "C" fn se_stage_run__settle() -> u32 {
    unsafe {
        let n = (&*(arena::LOOK as *const Look)).bodies.min(pool(arena::POOL_BODIES).cap);
        let ct = col(arena::COL_TRANSFORM);
        let mut i = 0u32;
        while i < n {
            (&mut *row::<Transform>(&ct, i)).rot = [0.0, 0.0, 0.0, 1.0];
            i += 1;
        }
        n
    }
}
"#;
const RENDER: &str = r#"
/// It records; the host replays. One boundary crossing per frame, and no GPU
/// API reaches in here.
#[no_mangle]
pub extern "C" fn record() -> u32 {
    unsafe {
        let look = &*(arena::LOOK as *const Look);
        let ct = col(arena::COL_TRANSFORM);
        let n = look.bodies.min(pool(arena::POOL_BODIES).cap);
        let zero = Command {
            kind: 0, vertices: 0, shader: 0, column: u32::MAX, count: 0,
            uniform_off: 0, uniform_len: 0, mesh: u32::MAX, texture: u32::MAX,
            clear_r: 0.0, clear_g: 0.0, clear_b: 0.0, clear_a: 1.0,
        };
        let cmds = arena::COMMANDS as *mut Command;
        let bg = if look.graph == 0 { 0.02 } else { 0.10 };
        *cmds.add(0) = Command { kind: 0, clear_r: bg, clear_g: bg, clear_b: bg, ..zero };
        *cmds.add(1) = Command {
            kind: 1, column: arena::COL_TRANSFORM, count: n,
            uniform_off: arena::CAMERA, uniform_len: Camera::SIZE, ..zero
        };
        *cmds.add(2) = Command { kind: 3, ..zero };
        let _ = ct;
        let f = &mut *(arena::FRAME as *mut Frame);
        f.commands = arena::COMMANDS;
        f.count = 3;
        f.present = 0;
        f.epoch = f.epoch.wrapping_add(1);
        3
    }
}
"#;

struct Built {
    contract: Vec<u8>,
    game: Vec<u8>,
    process: Vec<u8>,
    render: Vec<u8>,
    dir: PathBuf,
}

fn build_all() -> Result<Built> {
    let (g, dir) = contract_dir("frame")?;
    Ok(Built {
        game: build(&dir, "game", Slot::Game, &format!("{}{GAME}", prelude()))?,
        process: {
            // A stage names components, not column indices: which index a
            // column gets is the contract's business.
            let section = embed(se_vm::stages::SECTION, &stage_section());
            build(&dir, "process", Slot::Process, &format!("{}{section}{PROCESS}", prelude()))?
        },
        render: build(&dir, "render", Slot::Render, &format!("{}{RENDER}", prelude()))?,
        contract: g.module,
        dir,
    })
}

#[test]
fn one_contract_three_modules_one_memory() -> Result<()> {
    require_wasm_target();
    let b = build_all()?;
    let mut vm = Vm::new(&b.contract, 64)?;
    vm.load(Slot::Game, "game", b.game)?;
    vm.load(Slot::Process, "process", b.process)?;
    vm.load(Slot::Render, "render", b.render.clone())?;

    // Seed a column the host owns. Row `i` is row `i`: no entity, no query.
    let orbit = vm.meta.column_index("orbit").expect("orbit is a component");
    let transform = vm.meta.column_index("transform").expect("transform");
    for i in 0..8u32 {
        let at = vm.world.row_at(orbit, i);
        let mut bytes = Vec::new();
        // orbit: center vec3 @0, radius @12, speed @16, phase @20, tilt @24
        for f in [i as f32, 0.0, 0.0, 2.0 + i as f32, 0.5, 0.25, 1.5] {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        vm.write(at, &bytes)?;
    }

    // Discovered from the module's own declaration, in its declared order.
    let found: Vec<&str> = vm.stages().iter().map(|s| s.name.as_str()).collect();
    assert_eq!(found, ["motion", "settle"], "stages are discovered, not listed");

    let (rows, cmds) = vm.frame(3)?;
    // game.wasm asked for 2 + 3 % 4 = 5 bodies, and nothing else decided it.
    assert_eq!(rows, 5, "the roster is a control var");
    assert_eq!(cmds, 3, "render recorded begin/draw/end");

    // The stage's writes are in the host's own column, uncopied.
    let t0 = vm.read(vm.world.row_at(transform, 0), 48)?;
    let px = f32::from_le_bytes(t0[0..4].try_into()?);
    assert_eq!(px, 2.0, "pos.x = center.x + radius");
    let scale = f32::from_le_bytes(t0[32..36].try_into()?);
    assert_eq!(scale, 1.5, "scale is at offset 32, as std430 placed it");
    Ok(())
}

#[test]
fn replacing_the_control_module_leaves_the_world_alone() -> Result<()> {
    require_wasm_target();
    let b = build_all()?;
    let mut vm = Vm::new(&b.contract, 64)?;
    vm.load(Slot::Game, "game", b.game)?;
    vm.load(Slot::Process, "process", b.process)?;

    let orbit = vm.meta.column_index("orbit").unwrap();
    // A row is its whole stride, tail padding included: std430 rounds `orbit`
    // up to 32 while its seven scalars only reach 28. Seeding 28 and comparing
    // 32 would fail on the padding rather than on the data.
    let stride = vm.world.stride[orbit as usize] as usize;
    let mut seeded = Vec::new();
    for i in 0..8u32 {
        let mut bytes = Vec::new();
        for f in [i as f32, 1.0, 2.0, 3.0 + i as f32, 0.5, 0.25, 1.0] {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes.resize(stride, 0);
        vm.write(vm.world.row_at(orbit, i), &bytes)?;
        seeded.push(bytes);
    }
    let (rows_before, _) = vm.frame(1)?;

    // A different control module, asking for a different roster.
    let v2 = GAME.replace("look.bodies = 2 + n % 4;", "look.bodies = 7;");
    let bytes = build(&b.dir, "game2", Slot::Game, &format!("{}{v2}", prelude()))?;
    vm.load(Slot::Game, "game2", bytes)?;
    let (rows_after, _) = vm.frame(1)?;

    assert_eq!(rows_before, 3, "2 + 1 % 4");
    assert_eq!(rows_after, 7, "the replacement asked for seven");
    for (i, want) in seeded.iter().enumerate() {
        let back = vm.read(vm.world.row_at(orbit, i as u32), stride as u32)?;
        assert_eq!(&back, want, "row {i} survived the swap");
    }
    Ok(())
}

#[test]
fn a_module_that_imports_anything_else_is_refused() -> Result<()> {
    require_wasm_target();
    let b = build_all()?;
    let src = format!(
        "{}\nextern \"C\" {{ fn se_host_please(x: u32) -> u32; }}\n\
         #[no_mangle] pub extern \"C\" fn tick(n: u32) -> u32 {{ unsafe {{ se_host_please(n) }} }}\n",
        prelude()
    );
    let bytes = build(&b.dir, "rogue", Slot::Game, &src)?;
    let mut vm = Vm::new(&b.contract, 16)?;
    let err = vm.load(Slot::Game, "rogue", bytes).unwrap_err().to_string();
    assert!(
        err.contains("may import only") && err.contains("se_host_please"),
        "the refusal must name what it refused: {err}"
    );
    Ok(())
}
