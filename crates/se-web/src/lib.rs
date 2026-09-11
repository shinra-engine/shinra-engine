//! The engine, as the wasm route runs it.
//!
//! Everything above the loader is the same code the `.so` route runs — ecs,
//! schedule, registry, the graph description, and wgpu, which targets WebGPU
//! here and Vulkan there. What is route-specific is how a module arrives, and
//! that is `module.rs`.

#[cfg(target_arch = "wasm32")]
pub mod ctl;
#[cfg(target_arch = "wasm32")]
pub mod game;
#[cfg(target_arch = "wasm32")]
pub mod module;
#[cfg(target_arch = "wasm32")]
pub mod present;
#[cfg(target_arch = "wasm32")]
pub mod render;
#[cfg(target_arch = "wasm32")]
pub mod stage;

#[cfg(target_arch = "wasm32")]
mod entry;
