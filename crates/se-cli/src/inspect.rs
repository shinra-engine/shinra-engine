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

/// The one field an Inspector row edits: which column, which row of it, and
/// which field of the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub col: u32,
    pub row: u32,
    pub field: usize,
}

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
        Ty::Span => format!("span({}, {})", u32_at(b, 0), u32_at(b, 4)),
        Ty::Vec(n) => {
            let v: Vec<String> =
                (0..n as usize).map(|k| format!("{:.3}", f32_at(b, k * 4))).collect();
            format!("[{}]", v.join(", "))
        }
    }
}

/// One row of the world, read across every column of its pool.
pub fn inspect(vm: &Vm, selected: usize) -> (Vec<(String, String)>, Vec<Option<Target>>) {
    let mut rows = Vec::new();
    let mut map: Vec<Option<Target>> = Vec::new();

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
            map.push(Some(Target { col: *col, row, field: fi }));
        }
    }
    (rows, map)
}

/// The Project panel: the layout rule, as it stands on disk. Alongside each
/// line goes the `(category, module)` it names, for the rows that can be
/// swapped (game and render) and `None` for everything else.
pub fn project_tree(
    src: &crate::discover::GameSrc,
    active: &[(String, String)],
) -> (Vec<String>, Vec<Option<(String, String)>>) {
    let mut out = vec![format!("{}/", src.name), "  contract.wit".into()];
    let mut ids: Vec<Option<(String, String)>> = vec![None, None];
    for cat in &src.categories {
        if cat.modules.is_empty() {
            continue;
        }
        let choose = !crate::build::is_additive(&cat.name) && cat.name != "asset";
        out.push(format!("  {}/{}", cat.name, if choose { "  (one of)" } else { "  (all of them)" }));
        ids.push(None);
        for m in &cat.modules {
            let on = choose && active.iter().any(|(c, n)| *c == cat.name && *n == m.name);
            out.push(format!("    {}.rs{}", m.name, if on { "  <-" } else { "" }));
            ids.push(choose.then(|| (cat.name.clone(), m.name.clone())));
        }
        for sh in &cat.shared {
            out.push(format!("    {sh}/  (shared)"));
            ids.push(None);
        }
    }
    (out, ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::GameSrc;

    fn tree(name: &str, files: &[&str]) -> GameSrc {
        let root = std::env::temp_dir().join(format!("se-inspect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("contract.wit"), "").unwrap();
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        GameSrc::open(&root).unwrap()
    }

    fn act(c: &str, n: &str) -> Vec<(String, String)> {
        vec![(c.into(), n.into())]
    }

    #[test]
    fn render_rows_carry_identity_and_one_marker() {
        let src = tree("render", &["render/solid.rs", "render/ghost.rs"]);
        let (lines, ids) = project_tree(&src, &act("render", "solid"));
        assert_eq!(lines.len(), ids.len());
        assert_eq!(ids.iter().flatten().count(), 2);
        let marked: Vec<_> = lines.iter().zip(&ids).filter(|(l, _)| l.ends_with("<-")).collect();
        assert_eq!(marked.len(), 1);
        assert_eq!(marked[0].1, &Some(("render".to_string(), "solid".to_string())));
    }

    #[test]
    fn process_rows_are_never_swappable() {
        let src = tree("process", &["process/a.rs", "process/b.rs"]);
        let (lines, ids) = project_tree(&src, &act("process", "a"));
        assert_eq!(lines.len(), ids.len());
        assert!(ids.iter().all(|i| i.is_none()));
        assert!(lines.iter().all(|l| !l.ends_with("<-")));
    }

    #[test]
    fn a_lone_game_module_is_still_listed() {
        let src = tree("game", &["game/g.rs"]);
        let (_, ids) = project_tree(&src, &act("game", "g"));
        assert_eq!(ids.iter().flatten().count(), 1);
    }
}
