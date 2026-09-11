//! Turning a module's registrations into the host's own definitions.
//!
//! Same shape as `sink.rs` on the `.so` route, and for the same reason: the
//! host keeps owned `String`s and `Vec`s so nothing it holds points into a
//! module it might drop.

use crate::registry::{BufferDef, FieldDef, LayoutDef};
use crate::reloc::Reloc;
use anyhow::{bail, Context, Result};
use se_abi::{BufferDesc, Layout, Registered};

/// Where a module said its registrations are, read out of its memory.
///
/// # Safety
/// `at` must be what a `se_register_*` answered with.
pub unsafe fn registered(r: &Reloc, at: u32) -> Result<Registered> {
    if at == 0 {
        return Ok(Registered::EMPTY);
    }
    r.get::<Registered>(at).copied().context("registration descriptor is outside the module")
}

fn check<T>(d: &Registered, what: &str) -> Result<()> {
    let want = core::mem::size_of::<T>() as u32;
    if d.len != 0 && d.stride != want {
        bail!("{what}: module registered {}-byte items, this engine reads {want}", d.stride);
    }
    Ok(())
}

/// # Safety
/// `at` must be what `se_register_layouts` answered with.
pub unsafe fn layouts(r: &Reloc, at: u32) -> Result<Vec<LayoutDef>> {
    let d = registered(r, at)?;
    check::<Layout>(&d, "se_register_layouts")?;
    let items: &[Layout] =
        r.array(d.ptr as u32, d.len).context("layouts run past the module's memory")?;

    let mut out = Vec::with_capacity(items.len());
    for l in items {
        let fields = r.slice(&l.fields).context("a layout's fields are outside the module")?;
        out.push(LayoutDef {
            name: r.str(&l.name).context("a layout's name is outside the module")?.to_string(),
            size: l.size,
            align: l.align,
            hash: l.hash,
            fields: fields
                .iter()
                .map(|f| {
                    Ok(FieldDef {
                        name: r.str(&f.name).context("a field's name is outside the module")?.to_string(),
                        ty: f.ty,
                        offset: f.offset,
                        count: f.count,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        });
    }
    Ok(out)
}

/// # Safety
/// `at` must be what `se_register_buffers` answered with.
pub unsafe fn buffers(r: &Reloc, at: u32) -> Result<Vec<BufferDef>> {
    let d = registered(r, at)?;
    check::<BufferDesc>(&d, "se_register_buffers")?;
    let items: &[BufferDesc] =
        r.array(d.ptr as u32, d.len).context("buffers run past the module's memory")?;
    items
        .iter()
        .map(|b| {
            Ok(BufferDef {
                name: r.str(&b.name).context("a buffer's name is outside the module")?.to_string(),
                extent: b.extent,
                width: b.width,
                height: b.height,
                format: b.format,
                count: b.count,
                sampled: b.sampled,
            })
        })
        .collect()
}

/// # Safety
/// `at` must be what `se_register_graph` answered with.
pub unsafe fn graphs(r: &Reloc, at: u32) -> Result<Vec<crate::registry::GraphDef>> {
    use crate::registry::{DrawDef, GraphDef, PassDef};
    use se_abi::{DrawKind, GraphDesc};

    let d = registered(r, at)?;
    check::<GraphDesc>(&d, "se_register_graph")?;
    let items: &[GraphDesc] =
        r.array(d.ptr as u32, d.len).context("graphs run past the module's memory")?;

    let text = |s: &se_abi::Str| -> Result<String> {
        Ok(r.str(s).context("a name is outside the module")?.to_string())
    };
    let some = |s: &se_abi::Str| -> Result<Option<String>> {
        Ok(Some(text(s)?).filter(|t| !t.is_empty()))
    };
    let names = |s: &se_abi::Slice<se_abi::Str>| -> Result<Vec<String>> {
        r.slice(s).context("a name list is outside the module")?.iter().map(text).collect()
    };

    let mut out = Vec::with_capacity(items.len());
    for g in items {
        let passes = r.slice(&g.passes).context("passes are outside the module")?;
        let edges = r.slice(&g.edges).context("edges are outside the module")?;
        out.push(GraphDef {
            name: text(&g.name)?,
            present: text(&g.present)?,
            passes: passes
                .iter()
                .map(|p| {
                    Ok(PassDef {
                        name: text(&p.name)?,
                        shader: text(&p.shader)?,
                        color: names(&p.color)?,
                        depth: some(&p.depth)?,
                        reads: names(&p.reads)?,
                        textures: names(&p.textures)?,
                        uniform_of: some(&p.uniform_of)?,
                        clear: p.clear,
                        load: p.load,
                        draw: DrawDef {
                            kind: p.draw.kind,
                            instance_of: match p.draw.kind {
                                DrawKind::Instanced => some(&p.draw.instance_of)?,
                                _ => None,
                            },
                            mesh: some(&p.draw.mesh)?,
                        },
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            edges: edges
                .iter()
                .map(|e| Ok((text(&e.from)?, text(&e.to)?)))
                .collect::<Result<Vec<_>>>()?,
        });
    }
    Ok(out)
}

/// # Safety
/// `at` must be what `se_register_assets` answered with.
pub unsafe fn assets(r: &Reloc, at: u32) -> Result<Vec<crate::registry::AssetDef>> {
    use crate::registry::AssetDef;
    use se_abi::AssetDesc;

    let d = registered(r, at)?;
    check::<AssetDesc>(&d, "se_register_assets")?;
    let items: &[AssetDesc] =
        r.array(d.ptr as u32, d.len).context("assets run past the module's memory")?;
    items
        .iter()
        .map(|a| {
            Ok(AssetDef {
                name: r.str(&a.name).context("an asset name is outside the module")?.to_string(),
                bytes: r
                    .slice(&a.bytes)
                    .context("asset bytes are outside the module")?
                    .to_vec(),
            })
        })
        .collect()
}
