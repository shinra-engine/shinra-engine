//! A module must build for the wasm pipeline from the same source.
//!
//! design.md: the two routes are independent — a native host never loads a
//! `.wasm`, and a wasm host never loads a `.so`. What they share is the
//! *source*: a game repo picks a side at build time and nothing in its code
//! changes.
//!
//! So the property worth holding is narrow and mechanical. The same `.rs`,
//! compiled for `wasm32-unknown-unknown`, must produce a module that exports
//! the entry points its host looks up.
//!
//! Imports are not *forbidden*. wasm is a replaceable-module system in its own
//! right — instantiating a module and binding it is the hot swap, and modules
//! importing from each other is how they compose. Banning imports would be the
//! `.so` route's constraint, where a module cannot call back into a host it
//! does not share an address space with, projected onto a route that has no
//! such problem.
//!
//! Control is where that stops being merely allowed and becomes the mechanism:
//! a wasm module cannot be *handed* the host, because a function pointer names
//! nothing across the boundary. So it imports it, and that is checkable.
//!
//! Skipped, not failed, when the target is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

fn have_target() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
}

/// Enough of the binary format to answer "what does this export, and what does
/// it need?". Anything more belongs to a wasm host, and this is not one.
mod wasm {
    pub struct Shape {
        pub exports: Vec<String>,
        pub imports: Vec<String>,
    }

    fn uleb(d: &[u8], i: &mut usize) -> u32 {
        let (mut r, mut s) = (0u32, 0u32);
        loop {
            let b = d[*i];
            *i += 1;
            r |= ((b & 0x7f) as u32) << s;
            s += 7;
            if b & 0x80 == 0 {
                return r;
            }
        }
    }

    fn name(d: &[u8], i: &mut usize) -> String {
        let n = uleb(d, i) as usize;
        let s = String::from_utf8_lossy(&d[*i..*i + n]).into_owned();
        *i += n;
        s
    }

    pub fn shape(d: &[u8]) -> Shape {
        assert_eq!(&d[..4], b"\0asm", "not a wasm module");
        let (mut exports, mut imports) = (Vec::new(), Vec::new());
        let mut i = 8;
        while i < d.len() {
            let id = d[i];
            i += 1;
            let size = uleb(d, &mut i) as usize;
            let end = i + size;
            let mut j = i;
            match id {
                2 => {
                    let n = uleb(d, &mut j);
                    for _ in 0..n {
                        let m = name(d, &mut j);
                        let f = name(d, &mut j);
                        imports.push(format!("{m}.{f}"));
                        j += 1;
                        uleb(d, &mut j);
                    }
                }
                7 => {
                    let n = uleb(d, &mut j);
                    for _ in 0..n {
                        let nm = name(d, &mut j);
                        j += 1;
                        uleb(d, &mut j);
                        exports.push(nm);
                    }
                }
                _ => {}
            }
            i = end;
        }
        exports.sort();
        Shape { exports, imports }
    }
}

fn build_for_wasm(source: &str, crate_name: &str, dir: &Path) -> PathBuf {
    let out = Command::new("cargo")
        .current_dir(repo())
        .args(["build", "-p", "se", "--target", "wasm32-unknown-unknown"])
        .output()
        .expect("running cargo");
    assert!(
        out.status.success(),
        "building `se` for wasm32 failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let wdeps = repo().join("target/wasm32-unknown-unknown/debug/deps");
    let hdeps = repo().join("target/debug/deps");
    let se = std::fs::read_dir(&wdeps)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("libse-") && n.ends_with(".rlib")
            })
        })
        .expect("no wasm libse-*.rlib");

    let root = dir.join(format!("{crate_name}.rs"));
    std::fs::write(&root, source).unwrap();
    let wasm = dir.join(format!("{crate_name}.wasm"));

    let o = Command::new("rustc")
        .args(["--edition", "2021", "--target", "wasm32-unknown-unknown"])
        .args(["--crate-type", "cdylib", "--crate-name", crate_name])
        // proc macros are host artefacts even for a wasm build, so both
        // dependency paths are needed. This is the one real build-system
        // difference between the routes.
        .arg("-L").arg(format!("dependency={}", wdeps.display()))
        .arg("-L").arg(format!("dependency={}", hdeps.display()))
        .arg("--extern").arg(format!("se={}", se.display()))
        .arg("-o").arg(&wasm)
        .arg(&root)
        .output()
        .expect("running rustc");
    assert!(
        o.status.success(),
        "wasm32 build of {crate_name} failed:\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    wasm
}

const BUNDLE: &str = r#"
se::abi_version!();
se::components! {
    Transform { pos: [f32; 3], rot: [f32; 4] }
}
se::buffers!(
    se::BufferDesc::screen("scene", se::Format::Rgba8Unorm).sampled(),
    se::BufferDesc::screen("depth", se::Format::Depth32Float),
);
"#;

const CONTROL: &str = r#"
se::components! { Transform { pos: [f32; 3] } }

fn tick(c: &mut se::Ctl, _f: &se::Frame) {
    let e = c.spawn();
    c.set(e, "Transform", &Transform { pos: [0.0; 3] });
    c.log("tick");
}

se::control! {
    name: "wb_control",
    slots: [],
    tick: tick,
}
"#;

const PROCESS: &str = r#"
se::abi_version!();
se::components! { Transform { pos: [f32; 3] } }

#[se::stage]
fn drift(t: &mut Transform, dt: f32) {
    t.pos[0] += dt;
}
"#;

#[test]
fn a_bundle_builds_for_wasm_and_exports_its_entry_points() {
    if !have_target() {
        eprintln!("skipping: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("se_wasm_backend_bundle");
    std::fs::create_dir_all(&dir).unwrap();
    let w = build_for_wasm(BUNDLE, "wb_bundle", &dir);
    let s = wasm::shape(&std::fs::read(&w).unwrap());

    for want in ["se_abi_version", "se_register_layouts", "se_register_buffers", "memory"] {
        assert!(s.exports.iter().any(|e| e == want), "missing `{want}` in {:?}", s.exports);
    }
}

#[test]
fn a_stage_exports_itself_on_wasm_where_the_link_section_cannot_hold_it() {
    if !have_target() {
        eprintln!("skipping: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("se_wasm_backend_process");
    std::fs::create_dir_all(&dir).unwrap();
    let w = build_for_wasm(PROCESS, "wb_process", &dir);
    let s = wasm::shape(&std::fs::read(&w).unwrap());

    // The divergence design.md records: a custom section on wasm must be plain
    // bytes, and `StageEntry` is a pointer, so each stage exports itself.
    assert!(
        s.exports.iter().any(|e| e == "se_stage__drift"),
        "no per-stage export: {:?}",
        s.exports
    );
}

#[test]
fn control_reaches_the_host_by_import_on_wasm_where_a_vtable_names_nothing() {
    if !have_target() {
        eprintln!("skipping: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("se_wasm_backend_control");
    std::fs::create_dir_all(&dir).unwrap();
    let w = build_for_wasm(CONTROL, "wb_control", &dir);
    let s = wasm::shape(&std::fs::read(&w).unwrap());

    assert!(
        s.exports.iter().any(|e| e == "se_register_control"),
        "not a control module: {:?}",
        s.exports
    );

    // The `.so` route hands a module a struct of the host's function
    // pointers. Here the same calls arrive the only way they can: named
    // imports the linker recorded and the host answers at instantiation.
    // One per method the source actually calls — the rest are pruned.
    for want in ["env.se_ctl_spawn", "env.se_ctl_set", "env.se_ctl_log"] {
        assert!(s.imports.iter().any(|i| i == want), "missing `{want}` in {:?}", s.imports);
    }
}
