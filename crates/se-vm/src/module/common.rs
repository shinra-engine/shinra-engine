//! Checks shared verbatim between the native and web module-loading paths.
//!
//! `check_contract_id` touches only `&[u8]`/`u64` — no `wasmtime` or `js_sys`
//! dependency — so, unlike the rest of `native.rs`/`web.rs`, it has no
//! platform-specific reason to be split and lives here instead.

use anyhow::{bail, Result};

/// A module is only ever meaningful against the contract it was built from:
/// every offset in it came from that table. The id rides in a section that
/// `shinra build` writes, so it is read from the file, before anything runs.
pub fn check_contract_id(name: &str, bytes: &[u8], want: u64) -> Result<()> {
    let Ok(found) = se_contract::wasm::section(bytes, se_contract::table::CONTRACT_ID_SECTION)
    else {
        bail!("`{name}` carries no contract id; it was not built by `shinra build` against this contract")
    };
    if found != want.to_le_bytes() {
        let got = <[u8; 8]>::try_from(found).map(u64::from_le_bytes);
        let shown = got.map_or_else(|_| "malformed".to_string(), |g| format!("{g:016x}"));
        bail!("`{name}` was built against contract {shown}; this world is {want:016x} — run `shinra build`")
    }
    Ok(())
}
