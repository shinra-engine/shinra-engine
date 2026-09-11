//! Cross-module checks that need no GPU.
//!
//! A bundle is a set of modules that were compiled separately and have to
//! agree. `rustc` proves each one obeys `data.rs`; nothing proved that a
//! render pass names a buffer `buffer.rs` actually declares, or that an
//! instanced draw names a component that exists. Those were found when the
//! graph was built, which means when a device existed, which means when
//! someone opened the IDE.
//!
//! They are contract questions, not rendering questions, so they can be
//! answered at build time — and a build-time answer is the difference between
//! a CI failure and a black frame with a validation error behind it.

#[cfg(not(target_arch = "wasm32"))]
use crate::bundle::Bundle;
use crate::registry::{BufferDef, GraphDef, LayoutDef, PassDef};
use anyhow::{bail, Result};
use se_abi::DrawKind;

/// Every complaint the checker has, rather than only the first.
///
/// A first-error checker turns one broken bundle into as many build-fix cycles
/// as it has mistakes. Collecting them costs a `Vec`.
#[derive(Default)]
pub struct Report {
    pub problems: Vec<String>,
}

impl Report {
    fn say(&mut self, where_: &str, what: String) {
        self.problems.push(format!("{where_}: {what}"));
    }

    pub fn is_empty(&self) -> bool {
        self.problems.is_empty()
    }

    /// Fail with every problem listed, or succeed silently.
    pub fn into_result(self) -> Result<()> {
        if self.problems.is_empty() {
            return Ok(());
        }
        bail!("{}", self.problems.join("\n"));
    }
}

/// Check a loaded bundle's modules against each other.
///
/// Takes a `.so` bundle because that is what holds modules on this route; the
/// checks themselves are in [`graph`] and are route-agnostic.
#[cfg(not(target_arch = "wasm32"))]
pub fn bundle(b: &Bundle) -> Report {
    let mut r = Report::default();
    graph(b.graph(), &b.buffers, &b.layouts, &mut r);
    r
}

/// A graph against the spaces and buffers the bundle declares.
pub fn graph(g: &GraphDef, buffers: &[BufferDef], layouts: &[LayoutDef], r: &mut Report) {
    let has_buffer = |n: &str| buffers.iter().any(|b| b.name == n);
    let sampled = |n: &str| buffers.iter().any(|b| b.name == n && b.sampled);

    // `present` names the buffer that reaches the screen, not a pass.
    if !has_buffer(&g.present) {
        r.say(
            &g.name,
            format!("presents `{}`, which buffer.rs does not declare", g.present),
        );
    } else if !g.passes.iter().any(|p| p.color.iter().any(|c| c == &g.present)) {
        r.say(
            &g.name,
            format!("presents `{}`, which no pass in it writes", g.present),
        );
    }

    for p in &g.passes {
        pass(p, &g.name, &has_buffer, &sampled, layouts, r);
    }

    // An edge naming a pass that does not exist orders nothing, silently.
    for (from, to) in &g.edges {
        for end in [from, to] {
            if !g.passes.iter().any(|p| &p.name == end) {
                r.say(&g.name, format!("edge names `{end}`, which is not a pass"));
            }
        }
    }
}

fn pass(
    p: &PassDef,
    graph: &str,
    has_buffer: &dyn Fn(&str) -> bool,
    sampled: &dyn Fn(&str) -> bool,
    layouts: &[LayoutDef],
    r: &mut Report,
) {
    let at = format!("{graph}/{}", p.name);

    if p.color.is_empty() {
        r.say(&at, "writes no colour buffer".into());
    }
    for b in p.color.iter().chain(p.depth.iter()) {
        if !has_buffer(b) {
            r.say(&at, format!("writes `{b}`, which buffer.rs does not declare"));
        }
    }
    for b in &p.reads {
        if !has_buffer(b) {
            r.say(&at, format!("reads `{b}`, which buffer.rs does not declare"));
        } else if !sampled(b) {
            // Reading an unsampled buffer is the cycle the design forbids:
            // only a sampled buffer is double-buffered, and only a
            // double-buffered one can be read while something writes it.
            r.say(
                &at,
                format!("reads `{b}`, which is not `.sampled()` — mark it, or the read is a cycle"),
            );
        }
    }
    if let Some(u) = &p.uniform_of {
        if !layouts.iter().any(|l| &l.name == u) {
            r.say(&at, format!("takes its uniform from `{u}`, which data.rs does not declare"));
        }
    }
    if p.draw.kind == DrawKind::Instanced {
        match &p.draw.instance_of {
            None => r.say(&at, "is an instanced draw that names no component".into()),
            Some(c) if !layouts.iter().any(|l| &l.name == c) => {
                r.say(&at, format!("draws `{c}`, which data.rs does not declare"));
            }
            Some(_) => {}
        }
    }
}
