//! The IDE's panels, built from the contract at run time.
//!
//! Nothing here knows a game. The host compiled no `contract.rs`; it read
//! `contract.wasm`, so it has every record's name, every field's name, type
//! and offset, and where each column sits. That is enough to name and decode
//! any value in any game's world — which is what makes one editor binary an
//! editor for all of them.
//!
//! There is no entity to list. A pool is a run of rows and row `i` is row `i`
//! in every column of it, so the Hierarchy is pools and their rows, and the
//! Inspector is one row read across the columns that share its length.

use se_contract::layout::Ty;
use se_tui::Node;
use se_vm::Vm;

/// Which `(column, field)` an Inspector row edits, or `None` for a heading.
pub type RowMap = Vec<Option<(u32, usize)>>;

/// Pools, then their rows. A pool with no rows still shows, because "the
/// roster is empty" is a thing you need to be able to see.
pub fn hierarchy(vm: &Vm) -> Vec<Node> {
    let mut out = Vec::new();
    for (pi, p) in vm.meta.pools.iter().enumerate() {
        let len = vm.world.pool_len.get(pi).copied().unwrap_or(0);
        let cap = vm.world.pool_cap.get(pi).copied().unwrap_or(0);
        out.push(Node {
            label: format!("{} [{len}/{cap}]", p.name),
            components: Vec::new(),
        });
        for row in 0..len {
            out.push(Node {
                label: format!("  {}[{row}]", p.name),
                components: p
                    .columns
                    .iter()
                    .filter_map(|c| vm.meta.components.get(*c as usize))
                    .map(|r| r.name.clone())
                    .collect(),
            });
        }
    }
    if out.is_empty() {
        out.push(Node { label: "(no pools)".into(), components: Vec::new() });
    }
    out
}

/// Which pool and row a Hierarchy line refers to, walking the same order
/// `hierarchy` built.
fn locate(vm: &Vm, selected: usize) -> Option<(usize, u32)> {
    let mut i = 0usize;
    for (pi, _) in vm.meta.pools.iter().enumerate() {
        if i == selected {
            return None; // the pool heading itself
        }
        i += 1;
        let len = vm.world.pool_len.get(pi).copied().unwrap_or(0);
        for row in 0..len {
            if i == selected {
                return Some((pi, row));
            }
            i += 1;
        }
    }
    None
}

fn scalar(ty: Ty, b: &[u8]) -> String {
    fn f32_at(b: &[u8], i: usize) -> f32 {
        f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }
    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }
    match ty {
        Ty::F32 => format!("{:.3}", f32_at(b, 0)),
        Ty::U32 => format!("{}", u32_at(b, 0)),
        Ty::I32 => format!("{}", u32_at(b, 0) as i32),
        Ty::Vec(n) => {
            let v: Vec<String> =
                (0..n as usize).map(|k| format!("{:.3}", f32_at(b, k * 4))).collect();
            format!("[{}]", v.join(", "))
        }
    }
}

/// One row of the world, read across every column of its pool.
pub fn inspect(vm: &Vm, selected: usize) -> (Vec<(String, String)>, RowMap) {
    let mut rows = Vec::new();
    let mut map: RowMap = Vec::new();

    let Some((pi, row)) = locate(vm, selected) else {
        // Not on a row: show the control var set instead, which is the other
        // thing worth watching and the only thing a control module writes.
        for c in &vm.meta.control {
            rows.push((format!("{}", c.name), String::new()));
            map.push(None);
            let Some(rec) = vm.meta.record(&c.name) else { continue };
            let Ok(bytes) = vm.read(c.at, c.len) else { continue };
            for f in &rec.fields {
                let at = f.offset as usize;
                let val = bytes
                    .get(at..at + f.ty.size() as usize)
                    .map(|b| scalar(f.ty, b))
                    .unwrap_or_else(|| "?".into());
                rows.push((format!("  {}", f.name), val));
                map.push(None);
            }
        }
        return (rows, map);
    };

    let pool = &vm.meta.pools[pi];
    for col in &pool.columns {
        let Some(rec) = vm.meta.components.get(*col as usize) else { continue };
        rows.push((rec.name.clone(), String::new()));
        map.push(None);
        let at = vm.world.row_at(*col, row);
        let Ok(bytes) = vm.read(at, rec.size) else { continue };
        for (fi, f) in rec.fields.iter().enumerate() {
            let o = f.offset as usize;
            let val = bytes
                .get(o..o + f.ty.size() as usize)
                .map(|b| scalar(f.ty, b))
                .unwrap_or_else(|| "?".into());
            rows.push((format!("  {}", f.name), val));
            map.push(Some((*col, fi)));
        }
    }
    (rows, map)
}

/// The Project panel: the layout rule, as it stands on disk.
pub fn project_tree(src: &crate::discover::GameSrc) -> Vec<String> {
    let mut out = vec![format!("{}/", src.name), "  contract.wit".into()];
    for cat in &src.categories {
        if cat.modules.is_empty() {
            continue;
        }
        let additive = crate::build::is_additive(&cat.name);
        out.push(format!("  {}/{}", cat.name, if additive { "  (all of them)" } else { "  (one of)" }));
        for m in &cat.modules {
            out.push(format!("    {}.rs", m.name));
        }
        for sh in &cat.shared {
            out.push(format!("    {sh}/  (shared)"));
        }
    }
    out
}
