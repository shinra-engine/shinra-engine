//! The host end of the six registration points.
//!
//! A module builds its registrations in its own memory and returns where they
//! are; the host reads them and deep-copies out. The copy is what makes hot
//! swap work — by the time these functions return, the host holds nothing that
//! points into a library it might drop.
//!
//! The direction used to be the other way round, with the host handing in a
//! callback per item. That could never work for wasm, where a host function
//! pointer inside the module is a number naming nothing. Reading back works
//! for both: on a `.so` the pointer is an address, on wasm the same integer is
//! an offset into the module's linear memory.

use crate::loader::Module;
use crate::registry::{AssetDef, BufferDef, ControlDef, GraphDef, LayoutDef, StageDef};
use anyhow::Result;
use se_abi::{sym, AssetDesc, BufferDesc, ControlSpec, GraphDesc, Layout, StageSpec};

macro_rules! read {
    ($fname:ident, $abi:ident, $def:ident, $symbol:expr) => {
        /// # Safety
        /// `m` must stay loaded for the duration of the call. Nothing in the
        /// result borrows from it afterwards.
        pub unsafe fn $fname(m: &Module) -> Result<Vec<$def>> {
            Ok(m.items::<$abi>($symbol)?.iter().map(|i| $def::from_abi(i)).collect())
        }
    };
}

read!(layouts, Layout, LayoutDef, sym::LAYOUTS);
read!(buffers, BufferDesc, BufferDef, sym::BUFFERS);
read!(assets, AssetDesc, AssetDef, sym::ASSETS);
read!(stages, StageSpec, StageDef, sym::STAGES);
read!(graphs, GraphDesc, GraphDef, sym::GRAPH);
read!(controls, ControlSpec, ControlDef, sym::CONTROL);
