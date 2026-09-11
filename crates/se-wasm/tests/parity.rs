//! The same source, built both ways, must describe the same contract.
//!
//! This is the only honest check on `decode`'s offsets. They are wasm32 struct
//! layouts written out by hand, and hand-written offsets are wrong silently —
//! a field read four bytes early still parses, it just yields a number nobody
//! notices is meaningless. Comparing against the `.so` the engine already
//! trusts turns that into a failure.
//!
//! Skipped, not failed, when the wasm32 target is not installed: the check is
//! about the engine, not about who has run `rustup target add`.


mod common;
use common::{build_both, ensure_se_for_wasm, have_wasm_target};

const SOURCE: &str = r#"
se::abi_version!();

se::components! {
    Transform {
        pos: [f32; 3],
        rot: [f32; 4],
        scale: [f32; 3],
    }

    Body {
        vel: [f32; 3],
        mass: f32,
        alive: u32,
    }
}

se::buffers!(
    se::BufferDesc::screen("scene", se::Format::Rgba8Unorm).sampled(),
    se::BufferDesc::screen("depth", se::Format::Depth32Float),
    se::BufferDesc::fixed("shadow", 2048, 2048, se::Format::R32Float),
);
"#;

const RENDER_SOURCE: &str = r#"
se::graph!("solid", |g| g
    .present("scene")
    .pass("bodies", |p| p
        .shader("// wgsl goes here\nfn vs_main() {}\nfn fs_main() {}")
        .color(&["scene"])
        .depth("depth")
        .reads(&["trail"])
        .uniform_of("Camera")
        .clear([0.1, 0.2, 0.3, 1.0])
        .instanced("Transform", "model.obj"))
    .pass("post", |p| p
        .shader("fn vs_main() {}\nfn fs_main() {}")
        .color(&["trail"])
        .load())
    .edge("bodies", "post"));
"#;

const PROCESS_SOURCE: &str = r#"
se::abi_version!();

se::components! {
    Transform { pos: [f32; 3], rot: [f32; 4], scale: [f32; 3] }
    Body { vel: [f32; 3], mass: f32, alive: u32 }
}

#[se::stage]
fn drift(t: &mut Transform, b: &Body, dt: f32) {
    t.pos[0] += b.vel[0] * dt;
}

#[se::stage]
fn settle(b: &mut Body, i: &se::Input) {
    let _ = i.mouse_down;
    b.alive = 1;
}
"#;

/// A graph is pure data, so both backends must produce the same `GraphDef` —
/// shader source, clear colour, edges and all.
#[test]
fn a_graph_crosses_both_backends_unchanged() {
    if !have_wasm_target() {
        eprintln!("skipping: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("se_wasm_parity_graph");
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("render_root.rs");
    // `se::graph!` stamps the ABI version itself; a second one collides.
    std::fs::write(&root, RENDER_SOURCE).unwrap();

    ensure_se_for_wasm();
    let built = build_both(&root, "parity_graph", &dir);

    let m = se_host::Module::open(&built.so).unwrap();
    let so = unsafe { se_host::sink::graphs(&m) }.unwrap();

    let engine = se_wasm::engine().unwrap();
    let mut w = se_wasm::WasmModule::open(&engine, &built.wasm).unwrap();
    let wasm = w.graphs().unwrap();

    assert_eq!(so.len(), 1, "the .so side did not read what was written");
    assert_eq!(fmt_graphs(&so), fmt_graphs(&wasm));
}

/// A stage's description has to survive too, or `shinra check` could not vet a
/// wasm bundle before running it.
#[test]
fn stage_descriptions_cross_both_backends_unchanged() {
    if !have_wasm_target() {
        eprintln!("skipping: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("se_wasm_parity_stage");
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("process_root.rs");
    std::fs::write(&root, PROCESS_SOURCE).unwrap();

    ensure_se_for_wasm();
    let built = build_both(&root, "parity_stage", &dir);

    let m = se_host::Module::open(&built.so).unwrap();
    let mut so = unsafe { se_host::sink::stages(&m) }.unwrap();
    so.sort_by(|a, b| a.name.cmp(&b.name));

    let engine = se_wasm::engine().unwrap();
    let mut w = se_wasm::WasmModule::open(&engine, &built.wasm).unwrap();
    let wasm = w.stages().unwrap();

    assert_eq!(so.len(), 2, "the .so side did not collect both stages");
    let a: Vec<String> = so.iter().map(|s| fmt_params(&s.name, &s.params)).collect();
    let b: Vec<String> = wasm.iter().map(|s| fmt_params(&s.name, &s.params)).collect();
    assert_eq!(a, b);
}

fn fmt_params(name: &str, p: &[se_host::registry::ParamDef]) -> String {
    let ps: Vec<String> = p
        .iter()
        .map(|p| format!("{}:{:?}/{:?}/{}/{:#x}", p.name, p.kind, p.access, p.size, p.hash))
        .collect();
    format!("{name}({})", ps.join(", "))
}

fn fmt_graphs(v: &[se_host::registry::GraphDef]) -> Vec<String> {
    v.iter()
        .map(|g| {
            let ps: Vec<String> = g
                .passes
                .iter()
                .map(|p| {
                    format!(
                        "{}|{}|c={:?}|d={:?}|r={:?}|t={:?}|u={:?}|clear={:?}|load={}|{:?}/{:?}/{:?}",
                        p.name, p.shader, p.color, p.depth, p.reads, p.textures, p.uniform_of,
                        p.clear, p.load, p.draw.kind, p.draw.instance_of, p.draw.mesh
                    )
                })
                .collect();
            format!("{} present={} edges={:?} [{}]", g.name, g.present, g.edges, ps.join(" ;; "))
        })
        .collect()
}

#[test]
fn the_same_source_describes_the_same_contract_on_both_backends() {
    if !have_wasm_target() {
        eprintln!("skipping: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("se_wasm_parity");
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("bundle_root.rs");
    std::fs::write(&root, SOURCE).unwrap();

    ensure_se_for_wasm();
    let built = build_both(&root, "parity", &dir);

    // The .so side, through the engine's own loader.
    let m = se_host::Module::open(&built.so).unwrap();
    let so_layouts = unsafe { se_host::sink::layouts(&m) }.unwrap();
    let so_buffers = unsafe { se_host::sink::buffers(&m) }.unwrap();

    // The wasm side, through this crate.
    let engine = se_wasm::engine().unwrap();
    let mut w = se_wasm::WasmModule::open(&engine, &built.wasm).unwrap();
    let wasm_layouts = w.layouts().unwrap();
    let wasm_buffers = w.buffers().unwrap();

    assert_eq!(so_layouts.len(), 2, "the .so side did not read what was written");
    assert_eq!(fmt_layouts(&so_layouts), fmt_layouts(&wasm_layouts));
    assert_eq!(fmt_buffers(&so_buffers), fmt_buffers(&wasm_buffers));

    // The hash is what the whole contract turns on, so say so separately.
    for (a, b) in so_layouts.iter().zip(&wasm_layouts) {
        assert_eq!(a.hash, b.hash, "`{}` hashes differently across backends", a.name);
    }
}

fn fmt_layouts(v: &[se_host::registry::LayoutDef]) -> Vec<String> {
    v.iter()
        .map(|l| {
            let f: Vec<String> = l
                .fields
                .iter()
                .map(|f| format!("{}:{:?}[{}]@{}", f.name, f.ty, f.count, f.offset))
                .collect();
            format!("{} size={} align={} hash={:#x} {{{}}}", l.name, l.size, l.align, l.hash, f.join(","))
        })
        .collect()
}

fn fmt_buffers(v: &[se_host::registry::BufferDef]) -> Vec<String> {
    v.iter()
        .map(|b| {
            format!(
                "{} {:?} {}x{} {:?} count={} sampled={}",
                b.name, b.extent, b.width, b.height, b.format, b.count, b.sampled
            )
        })
        .collect()
}
