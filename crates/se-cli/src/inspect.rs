//! Reading the world back out for the editor panels.
//!
//! Everything here decodes from the *layout* rather than from any Rust type —
//! the host has none — which is why the Inspector can show, and now edit, a
//! component the engine has never seen. Split out of `run.rs` because the run
//! loop is about ordering a frame and this is about describing one.

use crate::session::Session;
use anyhow::{bail, Context, Result};
use se_tui::Node;

pub fn project_tree(s: &Session) -> Vec<String> {
    let mut out = vec![format!("v {}", s.bundle.name)];
    out.push("  bundle/".into());
    out.push("    data.rs".into());
    out.push("    buffer.rs".into());
    let mut cat = |name: &str, items: Vec<String>| {
        if items.is_empty() {
            return;
        }
        out.push(format!("  {name}/"));
        for i in items {
            out.push(format!("    {i}.so"));
        }
    };
    cat("asset", s.bundle.assets.iter().map(|l| l.module.name.clone()).collect());
    cat("process", s.bundle.process.iter().map(|l| l.module.name.clone()).collect());
    cat("render", vec![s.bundle.render.module.name.clone()]);
    cat("game", vec![s.bundle.control.module.name.clone()]);
    out
}

/// The Hierarchy panel: what exists, and what each thing carries.
pub fn hierarchy(s: &Session) -> Vec<Node> {
    let names: Vec<String> = s.world.component_names().into_iter().map(String::from).collect();
    s.world
        .entities()
        .iter()
        .enumerate()
        .map(|(i, &e)| {
            let mut components: Vec<String> = names
                .iter()
                .filter(|n| s.world.get(e, n).is_some())
                .cloned()
                .collect();
            components.sort();
            let label = match components.first() {
                Some(first) => format!("{first} #{i}"),
                None => format!("entity #{i}"),
            };
            Node { label, components }
        })
        .collect()
}

/// The Inspector panel: the selected entity's components, field by field,
/// decoded from the *layout* rather than from any Rust type — the host has
/// none, and that is exactly why this can show a component it has never seen.
/// Which component and field an Inspector row stands for. The IDE never sees
/// this — it draws strings — but the host needs it to turn typed text back
/// into bytes.
pub type RowMap = Vec<Option<(String, usize)>>;

pub fn inspect(s: &Session, index: usize) -> (Vec<(String, String)>, RowMap) {
    let Some(&e) = s.world.entities().get(index) else { return (Vec::new(), Vec::new()) };
    let mut rows = Vec::new();
    let mut map: RowMap = Vec::new();
    let mut names: Vec<String> = s.world.component_names().into_iter().map(String::from).collect();
    names.sort();
    for name in names {
        let Some(bytes) = s.world.get(e, &name) else { continue };
        let Some(layout) = s.world.layout(&name) else { continue };
        rows.push((format!("{name}:"), String::new()));
        map.push(None);
        for (i, f) in layout.fields.iter().enumerate() {
            rows.push((format!("  {}: ", f.name), field_text(bytes, f)));
            // A field wider than the value column shows is not offered for
            // editing: committing it would write back the truncation.
            map.push((f.count <= 4).then(|| (name.clone(), i)));
        }
    }
    (rows, map)
}

/// Parse what was typed into a field and write it into the store.
///
/// Refuses rather than guesses: a row must supply exactly as many values as
/// the field has, and each must parse as that field's scalar type. Half-writing
/// a vector because the third component had a typo would be worse than saying
/// so.
pub fn commit_field(
    s: &mut Session,
    index: usize,
    comp: &str,
    fi: usize,
    text: &str,
) -> Result<String> {
    use se_abi::ScalarTy;
    let Some(&e) = s.world.entities().get(index) else { bail!("no such entity") };
    let f = {
        let layout = s.world.layout(comp).context("component is not declared")?;
        layout.fields.get(fi).context("no such field")?.clone()
    };
    let parts: Vec<&str> = text.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.len() != f.count as usize {
        bail!("`{}` takes {} value(s), got {}", f.name, f.count, parts.len());
    }

    let mut buf: Vec<u8> = Vec::with_capacity(f.count as usize * f.ty.size() as usize);
    for p in &parts {
        match f.ty {
            ScalarTy::F32 => buf.extend_from_slice(&p.parse::<f32>()?.to_le_bytes()),
            ScalarTy::F64 => buf.extend_from_slice(&p.parse::<f64>()?.to_le_bytes()),
            ScalarTy::I32 => buf.extend_from_slice(&p.parse::<i32>()?.to_le_bytes()),
            ScalarTy::U32 => buf.extend_from_slice(&p.parse::<u32>()?.to_le_bytes()),
            ScalarTy::U8 => buf.push(p.parse::<u8>()?),
        }
    }

    let bytes = s.world.get_mut(e, comp).context("entity does not carry it")?;
    let off = f.offset as usize;
    let dst = bytes.get_mut(off..off + buf.len()).context("field runs past the component")?;
    dst.copy_from_slice(&buf);
    Ok(f.name)
}

pub fn field_text(bytes: &[u8], f: &se_host::registry::FieldDef) -> String {
    use se_abi::ScalarTy;
    let mut parts = Vec::new();
    for k in 0..f.count.min(4) {
        let off = (f.offset + k * f.ty.size()) as usize;
        let Some(b) = bytes.get(off..off + f.ty.size() as usize) else { break };
        parts.push(match f.ty {
            ScalarTy::F32 => format!("{:.3}", f32::from_le_bytes(b.try_into().unwrap())),
            ScalarTy::F64 => format!("{:.3}", f64::from_le_bytes(b.try_into().unwrap())),
            ScalarTy::I32 => i32::from_le_bytes(b.try_into().unwrap()).to_string(),
            ScalarTy::U32 => u32::from_le_bytes(b.try_into().unwrap()).to_string(),
            ScalarTy::U8 => b[0].to_string(),
        });
    }
    if f.count > 4 {
        parts.push("…".into());
    }
    parts.join(", ")
}
