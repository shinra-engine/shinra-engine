//! Saving and restoring the data, through the contract and nothing else.
//!
//! design.md gives `game.so` save/load so a game can carry progress across a
//! run: *"game.so 可以從data status s/l 去載入存檔遊戲進度"*. The interesting
//! constraint is the next line — *"改變bundle.so contract 等於替換game sets 需要
//! 全部重load"* — which is what makes a snapshot refusable rather than
//! best-effort. A file written under one contract is not partial data for
//! another one; it is data about a different game.
//!
//! Everything here goes through [`Store`]. Nothing reaches for a sparse set, a
//! generation counter or an entity id layout, which is both the reason a
//! snapshot survives swapping the data pipeline and a standing check that the
//! trait is actually complete: if saving needed something `Store` does not
//! expose, that would be a hole in the contract, not a reason to widen this
//! module's reach.

use crate::space::Store;
use anyhow::{bail, Context, Result};
use se_abi::Entity;
use std::path::Path;

const MAGIC: &[u8; 8] = b"SESNAP\0\x01";

/// Entities are renumbered on load, so the file stores a row's *position* in
/// its own entity table rather than the id it happened to have. A save from a
/// world that had recycled ids restores into one that has not, and neither
/// notices.
type Slot = u32;

// --- writing ----------------------------------------------------------------

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

/// Serialise every component of every live entity.
pub fn save(store: &dyn Store, contract: u64, path: &Path) -> Result<()> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u64(&mut out, contract);

    let ents: Vec<Entity> = store.entities().to_vec();
    put_u32(&mut out, ents.len() as u32);

    // Position in this table is the only entity identity the file has.
    let slot_of = |e: Entity| ents.iter().position(|&x| x == e).map(|i| i as Slot);

    // Sorted so two saves of the same state are the same bytes — a snapshot
    // that diffs is worth more than one that merely restores.
    let mut names: Vec<&str> = store.component_names();
    names.sort_unstable();

    let mut body = Vec::new();
    let mut n_written = 0u32;
    for name in names {
        let Some(layout) = store.layout(name) else { continue };
        let Some(col) = store.column(name) else { continue };
        let size = layout.size as usize;

        put_str(&mut body, name);
        put_u64(&mut body, layout.hash);
        put_u32(&mut body, layout.size);

        // Rows whose entity is not live are dropped rather than saved dead.
        let rows: Vec<(Slot, usize)> = col
            .entities()
            .iter()
            .filter_map(|&e| Some((slot_of(e)?, col.row_of(e)? as usize)))
            .collect();
        put_u32(&mut body, rows.len() as u32);

        let bytes = col.bytes();
        let stride = col.stride() as usize;
        for (slot, row) in rows {
            put_u32(&mut body, slot);
            let off = row * stride;
            let end = off + size;
            match bytes.get(off..end) {
                Some(b) => body.extend_from_slice(b),
                None => bail!("column `{name}` is shorter than the rows it reports"),
            }
        }
        n_written += 1;
    }

    put_u32(&mut out, n_written);
    out.extend_from_slice(&body);

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(path, &out).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

// --- reading ----------------------------------------------------------------

struct Cur<'a> {
    d: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let b = self.d.get(self.i..self.i + n).context("snapshot ends early")?;
        self.i += n;
        Ok(b)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn str(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        Ok(String::from_utf8(self.take(n)?.to_vec())?)
    }
}

/// Replace the store's contents with the snapshot's.
///
/// Refused outright when the contract differs: the caller's job then is to
/// reload the whole game set, which is what design.md asks for, and silently
/// restoring the components that happen to still match would defeat it.
pub fn load(store: &mut dyn Store, contract: u64, path: &Path) -> Result<()> {
    let d = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut c = Cur { d: &d, i: 0 };

    if c.take(8)? != MAGIC {
        bail!("{} is not a shinra snapshot", path.display());
    }
    let saved = c.u64()?;
    if saved != contract {
        bail!(
            "snapshot was written under contract {saved:#x}, this game set is {contract:#x} — \
             load the matching game set instead"
        );
    }

    // Parse the whole file before touching the store, so a truncated snapshot
    // leaves the running game alone instead of half-replacing it.
    let n_ents = c.u32()? as usize;
    let n_comps = c.u32()? as usize;
    let mut parsed: Vec<(String, u64, u32, Vec<(Slot, &[u8])>)> = Vec::with_capacity(n_comps);
    for _ in 0..n_comps {
        let name = c.str()?;
        let hash = c.u64()?;
        let size = c.u32()?;
        let n_rows = c.u32()? as usize;
        let mut rows = Vec::with_capacity(n_rows);
        for _ in 0..n_rows {
            let slot = c.u32()?;
            if slot as usize >= n_ents {
                bail!("snapshot names entity slot {slot} of {n_ents}");
            }
            rows.push((slot, c.take(size as usize)?));
        }
        parsed.push((name, hash, size, rows));
    }

    // A component whose shape moved is a contract break the file cannot
    // describe, even when the bundle hash matched.
    for (name, hash, size, _) in &parsed {
        match store.layout(name) {
            Some(l) if l.hash == *hash && l.size == *size => {}
            Some(_) => bail!("`{name}` has a different shape than the snapshot's"),
            None => bail!("snapshot has `{name}`, which this bundle does not define"),
        }
    }

    for e in store.entities().to_vec() {
        store.despawn(e);
    }
    let fresh: Vec<Entity> = (0..n_ents).map(|_| store.spawn()).collect();
    for (name, _, _, rows) in &parsed {
        for (slot, bytes) in rows {
            store.set(fresh[*slot as usize], name, bytes);
        }
    }
    Ok(())
}
