//! Region checks in `memory.rs`. Pure: no contract, no rustc.

use se_vm::memory::{check_base, check_heap, heap_limit, required_pages, Slot, STACK_SIZE};

#[test]
fn slot_bases_are_ordered_and_above_the_heap() {
    assert_eq!(Slot::Game.global_base(), 0x0080_0000);
    assert_eq!(Slot::Process.global_base(), 0x00C0_0000);
    assert_eq!(Slot::Render.global_base(), 0x0100_0000);
    assert!(Slot::Game.global_base() < Slot::Process.global_base());
    assert!(Slot::Process.global_base() < Slot::Render.global_base());
    for s in Slot::ALL {
        assert!(s.global_base() >= heap_limit());
    }
}

#[test]
fn required_pages_covers_the_top_slot() {
    assert_eq!(required_pages(), 0x0100_0000 / 0x1_0000 + 0x40);
    assert_eq!(required_pages(), 320);
}

#[test]
fn check_base_accepts_only_the_stack_size() {
    assert!(check_base(STACK_SIZE).is_ok());
    assert_eq!(STACK_SIZE, 0x10000);
    let msg = check_base(0x8000).unwrap_err().to_string();
    assert!(msg.contains("0x00008000"), "{msg}");
    assert!(msg.contains("0x00010000"), "{msg}");
    assert!(check_base(0x20000).is_err());
}

#[test]
fn heap_limit_is_the_game_base() {
    assert_eq!(heap_limit(), Slot::Game.global_base());
}

#[test]
fn check_heap_boundaries() {
    let limit = heap_limit();
    assert!(check_heap(limit - 8, 8).is_ok());
    assert!(check_heap(limit - 8, 9).is_err());
    assert!(check_heap(u32::MAX, 5).is_err());
    assert!(check_heap(0, u32::MAX).is_err());
}
