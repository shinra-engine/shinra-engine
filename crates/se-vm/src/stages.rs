//! Checking a process module's stages against the contract.
//!
//! The section format itself lives in `se_contract::stages`, because
//! `#[se::stage]` writes it and this reads it, and a format with two
//! implementations has two behaviours. What is here is the part that needs a
//! contract to check against — and it is the only thing standing between
//! "any module may bring any stage" and "unsound".

pub use se_contract::stages::{
    decode, encode, pad4, Param, Stage, MAGIC, READ, RUN_PREFIX, SECTION, VERSION, WRITE,
};

use crate::world::Meta;
use anyhow::{bail, Result};

/// Resolve each parameter to a column, and refuse anything that does not fit.
///
/// `se-host/src/schedule.rs` did this against a `Store`; the check is the same
/// one and the reason is the same, but what it checks against is now the
/// contract table, because there is no store left to ask.
pub fn resolve(stages: &mut [Stage], meta: &Meta) -> Result<()> {
    for s in stages.iter_mut() {
        if s.params.is_empty() {
            bail!(
                "stage `{}` names no column. A stage's parameter list is its query; \
                 one that names nothing would run over nothing.",
                s.name
            )
        }
        let mut seen: Vec<&str> = Vec::new();
        for p in s.params.iter_mut() {
            let Some(col) = meta.column_index(&p.component) else {
                bail!(
                    "stage `{}` wants `{}`, which the contract does not declare. \
                     The contract closes the set of types; a stage may only name one of them.",
                    s.name,
                    p.component
                )
            };
            if seen.contains(&p.component.as_str()) {
                bail!("stage `{}` names `{}` twice", s.name, p.component)
            }
            seen.push(&p.component);
            if p.access != READ && p.access != WRITE {
                bail!("stage `{}` gives `{}` access {}", s.name, p.component, p.access)
            }
            p.column = col;
        }
        // Row `i` means the same thing only within a pool, so a stage that
        // crossed one would be pairing unrelated rows.
        let pools: Vec<u32> =
            s.params.iter().map(|p| meta.pool_of(p.column)).collect::<Result<_>>()?;
        if pools.windows(2).any(|w| w[0] != w[1]) {
            let names: Vec<&str> = s.params.iter().map(|p| p.component.as_str()).collect();
            bail!(
                "stage `{}` names columns from more than one pool: {}. Row `i` means the \
                 same thing only within a pool.",
                s.name,
                names.join(", ")
            )
        }
    }
    Ok(())
}
