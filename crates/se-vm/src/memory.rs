//! The one linear memory every module imports, and the map over it.
//!
//! # Why one memory
//!
//! design.md: *兩個wasm可以用shared memory溝通 這是一切的基礎*. A module that
//! owns its memory can only be handed data by copying it in and copying the
//! results back out, which is what the old wasm route did per stage per frame.
//! Importing one memory the host owns removes both copies and, more
//! importantly, means the world is not inside any module — so replacing a
//! module cannot disturb it.
//!
//! # The region map, and the one invariant that bites
//!
//! ```text
//!   0x00000000  every module's stack   [0, STACK_SIZE)   grows down, traps at 0
//!   BASE        control vars, tables, then the host-managed heap
//!   +           one static/stack region per module with code, by --global-base
//! ```
//!
//! `BASE` must equal the `-z stack-size` every module is linked with.
//! Measured, not assumed: `--global-base` moves a module's *static data* and
//! leaves its stack alone, and rustc always passes `--stack-first`, so every
//! module's stack is the same `[0, stack-size)`. A world placed below that is
//! destroyed once a call nests deep enough — silently, and only at depth.
//! Placed correctly, an overflow traps instead, which is the failure you want.
//!
//! The shared stack region is only safe because no two modules are ever on the
//! stack at once. That is what the zero-import rule buys: a module that cannot
//! call out cannot be suspended inside another one. If any module ever gains
//! an import that re-enters another module, this stops being true.

use anyhow::{bail, Result};

/// Uniform stack size, and therefore the arena base. Every module must be
/// linked with `-z stack-size=` this, and the contract's `BASE` must match.
pub const STACK_SIZE: u32 = 0x0001_0000;

/// Where each module with code gets its statics. Assigned by kind rather than
/// by load order, so any `game.wasm` drops into the game slot and a replacement
/// lands on the same addresses as the module it replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Game,
    Process,
    Render,
}

impl Slot {
    /// `--global-base` for this slot. Decimal at the command line: lld refuses
    /// hex here.
    pub const fn global_base(self) -> u32 {
        match self {
            Slot::Game => 0x0080_0000,
            Slot::Process => 0x00C0_0000,
            Slot::Render => 0x0100_0000,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Slot::Game => "game",
            Slot::Process => "process",
            Slot::Render => "render",
        }
    }

    pub const ALL: [Slot; 3] = [Slot::Game, Slot::Process, Slot::Render];
}

/// Pages of 64 KiB the memory must have for every slot to exist. A module
/// records its `--global-base` as the minimum size of the memory it imports,
/// so wasmtime refuses to instantiate one whose region is not there — which
/// makes this a checked property rather than a convention.
pub fn required_pages() -> u32 {
    let top = Slot::ALL.iter().map(|s| s.global_base()).max().unwrap_or(0);
    // Room for the slot's own statics, stack and heap above its base.
    (top / 0x1_0000) + 0x40
}

/// Check the contract's arena base against the stack size the modules will be
/// linked with. Getting these out of step is the failure this whole module
/// exists to prevent, so it is refused at load rather than debugged at depth.
pub fn check_base(contract_base: u32) -> Result<()> {
    if contract_base != STACK_SIZE {
        bail!(
            "the contract places the arena at 0x{contract_base:08x} but every module's \
             stack occupies [0, 0x{STACK_SIZE:08x}). Anything below the stack top is \
             inside the stack and will be destroyed at depth — regenerate the contract \
             with a base of 0x{STACK_SIZE:08x}."
        )
    }
    Ok(())
}

/// Where the arena may not reach: a slot's own region.
pub fn heap_limit() -> u32 {
    Slot::ALL.iter().map(|s| s.global_base()).min().unwrap_or(u32::MAX)
}

/// Refuse a heap that would run into the first module's statics.
pub fn check_heap(heap: u32, wanted: u32) -> Result<()> {
    let limit = heap_limit();
    let end = heap.saturating_add(wanted);
    if end > limit {
        bail!(
            "the world needs 0x{wanted:x} bytes from 0x{heap:08x}, which reaches \
             0x{end:08x} — past 0x{limit:08x}, where the first module's statics live"
        )
    }
    Ok(())
}
