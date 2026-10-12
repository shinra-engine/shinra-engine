//! `World::place` and `put_blob`: placement arithmetic and the blob allocator.
//! No wasm module, no GPU — only the contract.

mod common;

use common::game1;
use se_vm::memory::heap_limit;
use se_vm::world::{COLUMN_BYTES, POOL_BYTES, SPAN_BYTES};
use se_vm::Vm;

const ROWS: u32 = 16;

fn vm() -> Vm {
    Vm::new(&game1().module, ROWS).expect("vm")
}

fn u32_at(vm: &Vm, at: u32) -> u32 {
    let b = vm.read(at, 4).expect("read");
    u32::from_le_bytes(b.try_into().unwrap())
}

fn align16(x: u32) -> u32 {
    (x + 15) & !15
}

#[test]
fn columns_are_placed_back_to_back_from_the_heap() {
    let vm = vm();
    let (m, w) = (&vm.meta, &vm.world);
    assert!(m.components.len() >= 2);
    assert_eq!(w.base[0], m.heap);
    for i in 1..m.components.len() {
        assert!(w.base[i] > w.base[i - 1]);
        assert_eq!(w.base[i], w.base[i - 1] + m.components[i - 1].size * ROWS);
    }
    let last = m.components.len() - 1;
    assert_eq!(w.blob_top, w.base[last] + m.components[last].size * ROWS);
    assert!(w.blob_top <= heap_limit());
}

#[test]
fn column_descriptors_are_published() {
    let vm = vm();
    let m = &vm.meta;
    for (i, r) in m.components.iter().enumerate() {
        let d = m.columns_at + i as u32 * COLUMN_BYTES;
        assert_eq!(u32_at(&vm, d), vm.world.base[i]);
        assert_eq!(u32_at(&vm, d + 4), ROWS);
        assert_eq!(u32_at(&vm, d + 8), r.size);
        assert_eq!(u32_at(&vm, d + 12), vm.world.pool_of[i]);
    }
    for pi in 0..m.pools.len() as u32 {
        let d = m.pools_at + pi * POOL_BYTES;
        assert_eq!(u32_at(&vm, d), 0);
        assert_eq!(u32_at(&vm, d + 4), ROWS);
    }
}

#[test]
fn a_world_larger_than_the_heap_is_refused() {
    let err = match Vm::new(&game1().module, 0x10_0000) {
        Ok(_) => panic!("a 1M-row world must not fit"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("past"), "{err}");
}

#[test]
fn put_blob_aligns_and_publishes_the_span() {
    let mut vm = vm();
    let top = vm.world.blob_top;
    let (off0, len0) = vm.put_blob(0, b"hello world!").unwrap();
    assert_eq!(len0, 12);
    assert_eq!(off0 % 16, 0);
    assert!(off0 >= top);
    let d = vm.meta.blobs_at;
    assert_eq!((u32_at(&vm, d), u32_at(&vm, d + 4)), (off0, 12));
    assert_eq!(vm.read(off0, 12).unwrap(), b"hello world!");

    let (off1, len1) = vm.put_blob(1, b"abcde").unwrap();
    assert_eq!(len1, 5);
    assert_eq!(off1, align16(off0 + 12));
    let d = vm.meta.blobs_at + SPAN_BYTES;
    assert_eq!((u32_at(&vm, d), u32_at(&vm, d + 4)), (off1, 5));
    assert_eq!(vm.read(off1, 5).unwrap(), b"abcde");
    assert_eq!(vm.read(off0, 12).unwrap(), b"hello world!");
}

#[test]
fn put_blob_refuses_a_slot_past_the_contract() {
    let mut vm = vm();
    assert_eq!(vm.meta.blob_slots, 64);
    let err = vm.put_blob(64, b"x").unwrap_err().to_string();
    assert!(err.contains("past the 64"), "{err}");
}
