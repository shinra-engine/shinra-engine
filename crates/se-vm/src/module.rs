//! Loading a module, and refusing one that would break the memory model.
//!
//! Two checks matter more than the loading.
//!
//! A module must import **only** the shared memory. An import that can call
//! back into another module would put two modules on the one shared stack at
//! once, and `memory.rs` explains why that region is only safe while exactly
//! one module is ever on it. So the rule the design states — *game.wasm 只是
//! 更新 contract.wit 裡面的control var sets* — is enforced here rather than
//! trusted.
//!
//! And a module must export no memory of its own. One that does was built
//! without `--import-memory`, and everything it writes would land somewhere
//! the host never reads. That failure is silent, so it is caught at load.
//!
//! Loading needs a wasm engine, and native and web have different ones —
//! wasmtime on native, the browser's own `WebAssembly` on web, with no
//! `wasmtime::Engine` reachable from inside a wasm32 binary at all (see
//! story-229's T1 finding). So, same split as `mem.rs`: this file names the
//! two implementations and nothing else; the `cfg(target_arch)` lives on
//! each `pub mod`, not on this file.

pub mod common;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
#[cfg(target_arch = "wasm32")]
pub mod web;

pub use common::check_contract_id;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{with_budget, Loaded, Prepared, DEFAULT_FUEL};
#[cfg(target_arch = "wasm32")]
pub use web::{Loaded, Prepared};
