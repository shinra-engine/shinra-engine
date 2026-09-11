//! The checker has to reject as well as accept.
//!
//! Every case here was reachable before only by opening the IDE and reading a
//! wgpu validation error out of a black frame — or, for the unsampled read,
//! not at all: it produced a frame that was merely wrong.

use se_abi::{DrawKind, Extent, Format};
use se_host::check;
use se_host::registry::{BufferDef, DrawDef, GraphDef, LayoutDef, PassDef};

fn buffer(name: &str, sampled: bool) -> BufferDef {
    BufferDef {
        name: name.into(),
        extent: Extent::Screen,
        width: 0,
        height: 0,
        format: Format::Rgba8Unorm,
        count: 1,
        sampled,
    }
}

fn layout(name: &str) -> LayoutDef {
    LayoutDef { name: name.into(), size: 4, align: 4, fields: Vec::new(), hash: 1 }
}

fn pass(name: &str) -> PassDef {
    PassDef {
        name: name.into(),
        shader: "s".into(),
        color: vec!["scene".into()],
        depth: None,
        reads: Vec::new(),
        textures: Vec::new(),
        uniform_of: None,
        clear: [0.0; 4],
        load: false,
        draw: DrawDef { kind: DrawKind::Fullscreen, instance_of: None, mesh: None },
    }
}

fn graph(passes: Vec<PassDef>) -> GraphDef {
    GraphDef { name: "g".into(), passes, edges: Vec::new(), present: "scene".into() }
}

fn run(g: &GraphDef, bufs: &[BufferDef], lays: &[LayoutDef]) -> Vec<String> {
    let mut r = check::Report::default();
    check::graph(g, bufs, lays, &mut r);
    r.problems
}

#[test]
fn a_sound_graph_is_accepted() {
    let g = graph(vec![pass("main")]);
    assert!(run(&g, &[buffer("scene", false)], &[]).is_empty());
}

#[test]
fn writing_an_undeclared_buffer_is_caught() {
    let mut p = pass("main");
    p.color = vec!["nowhere".into()];
    let mut g = graph(vec![p]);
    g.present = "nowhere".into();
    let out = run(&g, &[buffer("scene", false)], &[]);
    assert!(out.iter().any(|m| m.contains("buffer.rs does not declare")), "{out:?}");
}

#[test]
fn reading_an_unsampled_buffer_is_caught() {
    // The one that produced no error at all before: an unsampled buffer is
    // single-buffered, so reading it while a pass writes it is the cycle the
    // design exists to forbid.
    let mut p = pass("main");
    p.reads = vec!["scene".into()];
    let g = graph(vec![p]);
    let out = run(&g, &[buffer("scene", false)], &[]);
    assert!(out.iter().any(|m| m.contains("not `.sampled()`")), "{out:?}");
}

#[test]
fn reading_a_sampled_buffer_is_fine() {
    let mut p = pass("main");
    p.reads = vec!["trail".into()];
    let g = graph(vec![p]);
    let out = run(&g, &[buffer("scene", false), buffer("trail", true)], &[]);
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn drawing_an_undeclared_component_is_caught() {
    let mut p = pass("main");
    p.draw = DrawDef {
        kind: DrawKind::Instanced,
        instance_of: Some("Ghost".into()),
        mesh: None,
    };
    let g = graph(vec![p]);
    let out = run(&g, &[buffer("scene", false)], &[layout("Transform")]);
    assert!(out.iter().any(|m| m.contains("data.rs does not declare")), "{out:?}");
}

#[test]
fn an_edge_naming_no_pass_is_caught() {
    let mut g = graph(vec![pass("main")]);
    g.edges = vec![("main".into(), "ghost".into())];
    let out = run(&g, &[buffer("scene", false)], &[]);
    assert!(out.iter().any(|m| m.contains("edge names `ghost`")), "{out:?}");
}

#[test]
fn presenting_a_buffer_nobody_writes_is_caught() {
    let mut g = graph(vec![pass("main")]);
    g.present = "trail".into();
    let out = run(&g, &[buffer("scene", false), buffer("trail", true)], &[]);
    assert!(out.iter().any(|m| m.contains("no pass in it writes")), "{out:?}");
}

#[test]
fn every_problem_is_reported_not_just_the_first() {
    // A first-error checker turns one broken bundle into as many build-fix
    // cycles as it has mistakes.
    let mut p = pass("main");
    p.color = vec!["nowhere".into()];
    p.reads = vec!["alsonowhere".into()];
    let mut g = graph(vec![p]);
    g.present = "nowhere".into();
    assert!(run(&g, &[buffer("scene", false)], &[]).len() >= 2);
}
