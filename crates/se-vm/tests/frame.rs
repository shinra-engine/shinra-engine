//! One contract, three modules, one memory — built and run here.
//!
//! The modules are compiled by this test, the way `se/tests/wasm_backend.rs`
//! already compiles one: a contract is only proved by something built against
//! it, and a fixture checked in as bytes would stop tracking the generator.

use anyhow::{bail, Result};
use se_vm::memory::{Slot, STACK_SIZE};
use se_vm::Vm;
use std::path::{Path, PathBuf};
use std::process::Command;

fn has_wasm_target() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
}

/// Exactly the flags a module must be built with. They are here, in the test,
/// because getting any one of them wrong is a silent failure at run time:
///   --import-memory   or the module writes into a memory nobody reads
///   --global-base     or two modules' statics land on each other
///   -z stack-size     or a module's stack reaches down into the world
fn build(dir: &Path, name: &str, slot: Slot, src: &str) -> Result<Vec<u8>> {
    let file = dir.join(format!("{name}.rs"));
    std::fs::write(&file, src)?;
    let out = dir.join(format!("{name}.wasm"));
    let o = Command::new("rustc")
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

fn prelude() -> &'static str {
    "#![no_std]\n\
     #[panic_handler]\n\
     fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }\n\
     include!(\"contract.rs\");\n\
     #[inline]\n\
     unsafe fn col(i: u32) -> Column { *((arena::COLUMNS + i * 16) as *const Column) }\n\
     #[inline]\n\
     unsafe fn pool(i: u32) -> Pool { *((arena::POOLS + i * 8) as *const Pool) }\n\
     #[inline]\n\
     unsafe fn row<T>(c: &Column, i: u32) -> *mut T { (c.base + i * c.stride) as *mut T }\n"
}

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

/// Rust puts a `#[link_section]` static into a wasm custom section verbatim,
/// which is exactly what a declaration is. `#[used]` keeps `--gc-sections`
/// from dropping it: nothing references it, because the host reads it out of
/// the file rather than from inside the module.
fn embed(section: &str, bytes: &[u8]) -> String {
    let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
    format!(
        "#[used]\n#[link_section = \"{section}\"]\npub static SECTION_{}: [u8; {}] = [{}];\n",
        section.replace('.', "_").to_uppercase(),
        bytes.len(),
        list.join(",")
    )
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
            uniform_off: 0, uniform_len: 0, pad: 0,
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

/// Cargo runs these tests on parallel threads, so a directory keyed only by
/// pid is shared — and rustc's intermediate objects then collide under names
/// it picks from the crate name. One directory per build.
fn scratch(what: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "se-vm-{what}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

fn build_all() -> Result<Built> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    let dir = scratch("frame");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("contract.rs"), &g.rust)?;
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
    if !has_wasm_target() {
        eprintln!("skipping: rustup target add wasm32-unknown-unknown");
        return Ok(());
    }
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
    if !has_wasm_target() {
        return Ok(());
    }
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
    if !has_wasm_target() {
        return Ok(());
    }
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
