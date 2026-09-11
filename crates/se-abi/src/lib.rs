//! What is left of the old ABI.
//!
//! Most of this crate has been superseded by `contract.wit`: layouts, buffers
//! and the arena map are generated now, and a record that crosses the boundary
//! carries no pointer at all. What remains here is the part `se-render` still
//! reads — buffer and graph descriptions — plus `Input`.
//!
//! Three things that used to live here are gone rather than ported:
//!
//!   * `Ctl` — a game's whole write surface is the control var set the
//!     contract declares, so there is nothing left for a vtable to carry.
//!   * `Entity` — design.md keeps only `process.wasm`; a column is dense and
//!     row `i` is row `i`, so there is nothing to identify.
//!   * `StageCall.rows` — the dense index table a query used to produce. With
//!     no query there is nothing to index.

#![no_std]

pub mod buffer;
pub mod data;
pub mod graph;
pub mod input;
pub mod prim;
pub mod reg;
pub mod stage;

pub use buffer::{AssetDesc, BufferDesc, Extent, Format};
pub use data::{hash_begin, hash_field, Field, Layout, ScalarTy, Schema};
pub use graph::{Draw, DrawKind, Edge, GraphDesc, Pass};
pub use input::{key, Frame, Input, Slot, SlotBind, KEY_COUNT};
pub use prim::{AbiVersion, Slice, Str, ABI_MAJOR, ABI_MINOR};
pub use reg::{Held, Registered};
pub use stage::{slot_of, Access, Column, Param, ParamKind, StageCall, StageSpec};

/// Symbol names the host looks up. Kept here so host and module cannot drift.
pub mod sym {
    pub const ABI_VERSION: &[u8] = b"se_abi_version";
    pub const ASSETS: &[u8] = b"se_register_assets";
    pub const STAGES: &[u8] = b"se_register_stages";
    pub const GRAPH: &[u8] = b"se_register_graph";
}
