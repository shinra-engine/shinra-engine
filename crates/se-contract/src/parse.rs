//! `contract.wit` in, a laid-out contract out.
//!
//! WIT is read with `wit-parser` rather than a dialect of our own, so a
//! contract is a real WIT file and every tool that reads WIT can read it. What
//! this module adds is the two things WIT deliberately does not have: a
//! layout (see [`crate::layout`]) and a meaning per interface.
//!
//! The interface a record lives in is not decoration — it is the permission:
//!
//! ```text
//! types       vec2/vec3/vec4 — primitives, never emitted
//! handles     span, column   — fixed-size references to variable-length things
//! components  per-row data   — written by a process stage that names it `&mut`
//! control     the control var set — written only by game.wasm
//! pools       which columns share a length and are row-aligned
//! render      command, frame — the glue; never seen by a shader
//! ```

use crate::layout::{lay_out, Record, Ty};
use anyhow::{bail, Context, Result};
use std::path::Path;
use wit_parser::{Resolve, Type, TypeDefKind, TypeId};

/// A pool: the columns that share one length and are row-aligned.
///
/// Row `i` means the same thing in every column of a pool, and a pool is
/// exactly what one `draw-instanced` uploads — `game3/render/vn.rs` draws
/// `Actor` and `Glyph` as two separate populations, which is why one global
/// length will not do.
#[derive(Clone, Debug)]
pub struct Pool {
    pub name: String,
    /// Component record names, in declaration order.
    pub columns: Vec<String>,
}

#[derive(Debug)]
pub struct Contract {
    pub records: Vec<Record>,
    pub pools: Vec<Pool>,
}

impl Contract {
    pub fn of<'a>(&'a self, iface: &'a str) -> impl Iterator<Item = &'a Record> + 'a {
        self.records.iter().filter(move |r| r.iface == iface)
    }

    pub fn component(&self, name: &str) -> Option<&Record> {
        self.records.iter().find(|r| r.iface == "components" && r.name == name)
    }

    /// Every component, in the order the arena will place them. Sorted so a
    /// column index is stable against unrelated edits to the WIT file.
    pub fn components(&self) -> Vec<&Record> {
        let mut v: Vec<&Record> = self.of("components").collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

/// Follow `use` aliases to the record a name really means.
fn chase(resolve: &Resolve, mut id: TypeId) -> Result<TypeId> {
    for _ in 0..32 {
        match &resolve.types[id].kind {
            TypeDefKind::Type(Type::Id(next)) => id = *next,
            _ => return Ok(id),
        }
    }
    bail!("`use` aliases nest more than 32 deep")
}

/// A field type, as something with a layout. Only the WGSL vector types and
/// the three 32-bit scalars: anything else has no agreed placement on both
/// sides of the boundary, so it is refused rather than guessed at.
fn resolve_ty(resolve: &Resolve, t: &Type) -> Result<Ty> {
    match t {
        Type::F32 => Ok(Ty::F32),
        Type::S32 => Ok(Ty::I32),
        Type::U32 => Ok(Ty::U32),
        Type::String => Ok(Ty::Span),
        Type::Id(id) => {
            let id = chase(resolve, *id)?;
            let td = &resolve.types[id];
            let name = td.name.clone().unwrap_or_default();
            if let TypeDefKind::List(elem) = &td.kind {
                return match resolve_ty(resolve, elem)? {
                    Ty::Span => bail!(
                        "a `list` of `string`, `list` or `span` is refused: its elements \
                         would be spans, and a span points at flat bytes only"
                    ),
                    _ => Ok(Ty::Span),
                };
            }
            let TypeDefKind::Record(r) = &td.kind else {
                bail!("`{name}` is not a record; a contract field must be a scalar or a vec")
            };
            if name == "span"
                && r.fields.len() == 2
                && r.fields[0].name == "off"
                && r.fields[1].name == "len"
                && r.fields.iter().all(|f| f.ty == Type::U32)
            {
                return Ok(Ty::Span);
            }
            if !r.fields.iter().all(|f| f.ty == Type::F32) {
                bail!("`{name}` is nested in a field but is not a vector of f32")
            }
            match (name.as_str(), r.fields.len()) {
                ("vec2", 2) | ("vec3", 3) | ("vec4", 4) => Ok(Ty::Vec(r.fields.len() as u32)),
                _ => bail!(
                    "`{name}` cannot be a field type. Use f32/s32/u32 or vec2/vec3/vec4 — \
                     a nested record has no layout both sides would agree on"
                ),
            }
        }
        other => bail!(
            "{other:?} has no contract layout. The contract carries no pointer and no \
             `usize`: those differ between wasm32 and the host. Variable length is a \
             `string`, a `list` or a `span`"
        ),
    }
}

/// A pool member: the *type* of the field names the column.
fn resolve_member(resolve: &Resolve, t: &Type) -> Result<String> {
    let Type::Id(id) = t else {
        bail!("a pool's members must be component types, not scalars")
    };
    let id = chase(resolve, *id)?;
    resolve.types[id]
        .name
        .clone()
        .context("a pool member has no name")
}

pub fn parse(path: &Path) -> Result<Contract> {
    let mut resolve = Resolve::default();
    let (pkg, _) = resolve
        .push_path(path)
        .with_context(|| format!("reading {}", path.display()))?;

    let mut records = Vec::new();
    let mut pools = Vec::new();

    for (iname, iid) in &resolve.packages[pkg].interfaces {
        // vec2/vec3/vec4 are primitives to this compiler, not records it emits.
        if iname == "types" {
            continue;
        }
        for (tname, tid) in &resolve.interfaces[*iid].types {
            let td = &resolve.types[*tid];
            let TypeDefKind::Record(r) = &td.kind else { continue };
            // A `use`d type is listed in the importing interface too; it is
            // already emitted where it was declared.
            if td.name.as_deref() != Some(tname.as_str()) {
                continue;
            }
            if matches!(tname.as_str(), "vec2" | "vec3" | "vec4") {
                continue;
            }

            if iname == "pools" {
                let mut columns = Vec::new();
                for f in &r.fields {
                    columns.push(
                        resolve_member(&resolve, &f.ty)
                            .with_context(|| format!("pool `{tname}`, member `{}`", f.name))?,
                    );
                }
                pools.push(Pool { name: tname.clone(), columns });
                continue;
            }

            let mut fields = Vec::with_capacity(r.fields.len());
            for f in &r.fields {
                let ty = resolve_ty(&resolve, &f.ty)
                    .with_context(|| format!("`{tname}.{}`", f.name))?;
                fields.push((f.name.clone(), ty));
            }
            records.push(lay_out(tname, iname, &fields));
        }
    }

    records.sort_by(|a, b| (&a.iface, &a.name).cmp(&(&b.iface, &b.name)));
    pools.sort_by(|a, b| a.name.cmp(&b.name));

    let contract = Contract { records, pools };
    check_pools(&contract)?;
    check_span_declared(&contract)?;
    Ok(contract)
}

/// The generated Rust names `Span`, and the host checks `handles.span` at
/// load, so a record that carries one needs the handle declared.
fn check_span_declared(c: &Contract) -> Result<()> {
    if c.of("handles").any(|r| r.name == "span") {
        return Ok(());
    }
    if let Some(r) = c.records.iter().find(|r| r.fields.iter().any(|f| f.ty == Ty::Span)) {
        bail!(
            "record `{}` carries a string/list/span field but `interface handles` \
             declares no `record span {{ off: u32, len: u32 }}`",
            r.name
        );
    }
    Ok(())
}

/// A pool may only name components, and every component should belong to
/// exactly one pool — a column nobody pools has no length and can never be
/// drawn.
fn check_pools(c: &Contract) -> Result<()> {
    let mut claimed: Vec<&str> = Vec::new();
    for p in &c.pools {
        for col in &p.columns {
            if c.component(col).is_none() {
                bail!(
                    "pool `{}` names `{col}`, which is not in `interface components`",
                    p.name
                );
            }
            if claimed.contains(&col.as_str()) {
                bail!("`{col}` is in two pools; a column has one length");
            }
            claimed.push(col);
        }
    }
    for r in c.components() {
        if !claimed.contains(&r.name.as_str()) {
            bail!(
                "component `{}` is in no pool, so it has no length and nothing could \
                 draw it. Add it to a pool in `interface pools`.",
                r.name
            );
        }
    }
    Ok(())
}
