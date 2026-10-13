//! What a game module compiles against.
//!
//! Almost nothing, now. The types a module needs — every component, every
//! control var, the arena map, the column and pool descriptors — are generated
//! from `contract.wit` and included into the module's own crate root, so they
//! are not imported from anywhere. What is left here is the one thing a
//! generator cannot supply: the attribute that turns a function into a stage.
//!
//! This crate used to carry the whole ABI: `Str`, `Slice`, `Layout`, `Ctl`,
//! `StageCall`, six registration entry points and an allocator. All of it is
//! gone rather than ported —
//!
//!   * layouts and buffers are `contract.wit`, and generated;
//!   * `Ctl` is gone because a game's whole write surface is the control var
//!     set the contract declares;
//!   * `StageCall` is gone because there is no query left to resolve: a column
//!     is dense and row `i` is row `i` within its pool;
//!   * the allocator is gone because a module allocates nothing — the world is
//!     in memory the host owns and every module imports.
//!
//! A module that needs none of those needs no ABI crate either, which is why
//! this one is four lines of code and a page of explanation.

#![no_std]

/// `#[se::stage]` — a function whose parameter list is its query.
///
/// ```ignore
/// #[se::stage]
/// fn orbit(o: &mut Orbit, t: &mut Transform) { ... }
/// ```
///
/// It emits the declaration the host reads out of the file, and the batched
/// loop that runs the body over every row of the pool.
pub use se_macro::stage;

/// `se::asset!("dir/model.obj")` — an asset module's whole body.
pub use se_macro::asset;

/// `#[derive(se::Schema)]` and `se::components!` — removed.
///
/// A component is declared in `contract.wit` and generated, so declaring one
/// from Rust would be the second place its shape is written down. Kept only so
/// an old module fails with a message naming `contract.wit` rather than an
/// unknown macro.
pub use se_macro::{components, Schema};
