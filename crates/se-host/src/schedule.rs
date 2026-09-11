//! Checking a stage against the contract before it is ever run.
//!
//! Scheduling used to live here too: intersect the columns a signature names
//! and write one row of dense indices per match. There is nothing left to
//! intersect. Columns are dense and row `i` is row `i` within a pool, so a
//! stage runs over `0..len` and the whole `resolve` step is gone.
//!
//! What remains is the gate that makes an open set of behaviours safe. The
//! contract closes the set of *types*; a process module may export any stage
//! at all, including ones written long after this engine was built, so long as
//! every column it names is one the contract declares at the shape it was
//! compiled against. That is checked here, once, at load.

use crate::registry::StageDef;
use crate::space::Space;
use anyhow::{bail, Result};
use se_abi::{Input, ParamKind};
use std::collections::HashSet;

/// Check a stage against the bundle contract before it is ever run.
pub fn validate(world: &dyn Space, s: &StageDef) -> Result<()> {
    let mut seen = HashSet::new();
    for p in &s.params {
        match p.kind {
            ParamKind::Component => {
                if !seen.insert(p.name.as_str()) {
                    bail!("stage `{}` names `{}` twice", s.name, p.name);
                }
                let Some(l) = world.layout(&p.name) else {
                    bail!("stage `{}` wants `{}`, which the bundle does not define", s.name, p.name)
                };
                if l.hash != p.hash {
                    bail!(
                        "stage `{}` was built against a different `{}` — rebuild it against the current bundle",
                        s.name,
                        p.name
                    );
                }
            }
            ParamKind::Resource => {
                if p.name != Input::NAME {
                    bail!("stage `{}` wants unknown resource `{}`", s.name, p.name);
                }
                if p.size as usize != std::mem::size_of::<Input>() {
                    bail!("stage `{}` was built against a different Input", s.name);
                }
            }
            ParamKind::Dt => {}
        }
    }
    Ok(())
}


