//! Turning what a module registered into what the host keeps.
//!
//! Both routes end in the same place: a `*const Registered` that names an
//! array in memory the host can read. How they get there differs — `dlopen`
//! and a symbol lookup on one side, `WebAssembly.instantiate` and a shared
//! linear memory on the other — but from the pointer onwards there is nothing
//! route-specific left, so it lives here and neither side reimplements it.
//!
//! On the wasm route the sharing is literal: a module is linked with
//! `--import-memory`, so it and the engine are in one address space and a
//! pointer out of a module is a pointer the engine can follow. That is wasm's
//! own dynamic linking, not an emulation of dlopen.

use anyhow::{bail, Result};
use se_abi::Registered;

/// The registered items as a slice, with the element size checked.
///
/// `stride` is what replaces the type that used to be baked into a sink's
/// function pointer: a module built against a different `se` is refused rather
/// than reinterpreted.
///
/// # Safety
/// `r` must be what a `se_register_*` returned, `T` must be the type that
/// symbol registers, and the module must still be loaded.
pub unsafe fn items<'a, T>(r: *const Registered, what: &str, who: &str) -> Result<&'a [T]> {
    let Some(r) = r.as_ref() else {
        // A null answer is "nothing registered", which is legitimate for an
        // optional entry point and the caller's problem for a required one.
        return Ok(&[]);
    };
    if r.len == 0 {
        return Ok(&[]);
    }
    let want = core::mem::size_of::<T>() as u32;
    if r.stride != want {
        bail!(
            "`{who}` registered {}-byte items where {what} expects {want} — \
             rebuild it against this engine",
            r.stride
        );
    }
    Ok(core::slice::from_raw_parts(r.ptr as *const T, r.len as usize))
}
