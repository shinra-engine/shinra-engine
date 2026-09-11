//! The Shinra host: it owns the clock and the module table, and it is the only
//! thing in the system that is not hot swappable.
//!
//! It does not own a world. The data lives behind [`Store`], so the ECS is as
//! replaceable as everything else — the host cannot name an implementation of
//! it, let alone depend on one.
//!
//! Everything the host knows about a game arrives at runtime through six entry
//! points. It has no types for any of it — only layouts, buffer descriptions,
//! stage specs and a graph — which is precisely why a module can be rebuilt
//! and reloaded underneath a world that keeps its contents.

pub mod read;
pub mod registry;
pub mod schedule;
pub mod space;

pub use space::{Records, Space};
