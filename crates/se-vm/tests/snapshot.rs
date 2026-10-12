//! `Vm::snapshot` and `Vm::restore`: the whole save format. No wasm module,
//! no GPU — only the contract, and `frame(0)` with every slot empty.

mod common;

use common::game1;
use se_vm::Vm;

const ROWS: u32 = 16;

fn vm() -> Vm {
    Vm::new(&game1().module, ROWS).expect("vm")
}

fn pattern(len: u32, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed)).collect()
}

fn column_len(vm: &Vm, c: usize) -> u32 {
    vm.meta.components[c].size * ROWS
}

#[test]
fn restore_of_a_snapshot_is_the_identity_on_columns_and_blob() {
    let mut vm = vm();
    assert!(vm.meta.components.len() >= 2);
    let cols: Vec<(u32, Vec<u8>)> = (0..2)
        .map(|c| (vm.world.base[c], pattern(column_len(&vm, c), c as u8 + 1)))
        .collect();
    for (at, p) in &cols {
        vm.write(*at, p).unwrap();
    }
    let blob = pattern(100, 9);
    let (bat, blen) = vm.put_blob(0, &blob).unwrap();
    let snap = vm.snapshot();

    for (at, p) in &cols {
        vm.write(*at, &vec![0xEE; p.len()]).unwrap();
    }
    vm.write(bat, &vec![0xEE; blen as usize]).unwrap();
    assert_ne!(vm.read(bat, blen).unwrap(), blob);

    vm.restore(&snap).unwrap();
    for (at, p) in &cols {
        assert_eq!(&vm.read(*at, p.len() as u32).unwrap(), p);
    }
    assert_eq!(vm.read(bat, blen).unwrap(), blob);
    assert_eq!(vm.snapshot(), snap);
}

#[test]
fn snapshot_length_is_blob_top_minus_base_and_grows_with_put_blob() {
    let mut vm = vm();
    let before = vm.snapshot().len() as u32;
    assert_eq!(before, vm.world.blob_top - vm.meta.base);

    let top = vm.world.blob_top;
    vm.put_blob(0, &[1u8; 100]).unwrap();
    let after = vm.snapshot().len() as u32;
    assert_eq!(after, vm.world.blob_top - vm.meta.base);
    let aligned = (top + 15) & !15;
    assert_eq!(after - before, (aligned - top) + 100);
}

#[test]
fn restore_does_not_resync_the_pool_len_mirror_until_frame() {
    let mut vm = vm();
    vm.set_len(0, 3).unwrap();
    assert_eq!(vm.world.pool_len[0], 3);
    let snap = vm.snapshot();

    vm.set_len(0, 0).unwrap();
    assert_eq!(vm.world.pool_len[0], 0);

    vm.restore(&snap).unwrap();
    // Memory holds 3 again, but restore writes memory only: the host mirror
    // is stale. A fix that resyncs on restore should flip this assertion.
    let at = vm.meta.pools_at;
    assert_eq!(vm.read(at, 4).unwrap(), 3u32.to_le_bytes());
    assert_eq!(vm.world.pool_len[0], 0);

    // Empty slots: frame only reconciles the mirror from memory.
    vm.frame(0).unwrap();
    assert_eq!(vm.world.pool_len[0], 3);
}
