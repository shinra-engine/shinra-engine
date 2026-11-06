//! The wasm host: one memory, a few modules, and a frame.
//!
//! This is the route design.md settles on — *在pc上使用winit + wasmtime* — and
//! it is deliberately not the `.so` route with a different loader. The
//! difference that matters is not dlopen versus instantiate; it is that the
//! world lives in a memory the host owns and every module imports, so
//! replacing a module cannot disturb it and no data is copied to hand it over.
//!
//! The host compiles no game's contract in. It reads `contract.wasm` at run
//! time, which is what lets one binary run any game — design.md: *像是短影音
//! 左滑一下就換了一款遊戲的感覺*.

pub mod clock;
pub mod input;
pub mod mem;
pub mod memory;
pub mod module;
pub mod save;
pub mod sound;
pub mod stages;
pub mod world;

mod control;
mod rollback;
mod vm;
pub use vm::Vm;
