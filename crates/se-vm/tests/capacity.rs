//! `Vm::frame` clamps a roster request to capacity and reports it.
//! No wasm module: an empty slot returns 0, so only the reconcile runs.

mod common;

use common::game1;
use se_vm::world::POOL_BYTES;
use se_vm::Vm;

const ROWS: u32 = 16;

fn ask(vm: &mut Vm, len: u32) {
    let at = vm.meta.pools_at;
    vm.write(at, &len.to_le_bytes()).expect("write");
}

fn arena_len(vm: &Vm) -> u32 {
    let b = vm.read(vm.meta.pools_at, 4).expect("read");
    u32::from_le_bytes(b.try_into().unwrap())
}

#[test]
fn overrun_is_clamped_and_reported_once() {
    let mut vm = Vm::new(&game1().module, ROWS).expect("vm");
    assert!(POOL_BYTES >= 4);
    ask(&mut vm, ROWS + 5);
    vm.frame(0).expect("frame");
    assert_eq!(vm.world.pool_len[0], ROWS);
    assert_eq!(arena_len(&vm), ROWS);
    let name = vm.meta.pools[0].name.clone();
    let o = vm.take_overruns();
    assert_eq!(o.len(), 1);
    assert!(o[0].contains(&format!("`{name}`")) && o[0].contains(&format!("{}", ROWS + 5)));

    // Asking again with the same overrun, before the list is taken.
    ask(&mut vm, ROWS + 5);
    vm.frame(1).expect("frame");
    ask(&mut vm, ROWS + 5);
    vm.frame(2).expect("frame");
    assert_eq!(vm.take_overruns().len(), 1);
}

#[test]
fn normal_length_reports_nothing() {
    let mut vm = Vm::new(&game1().module, ROWS).expect("vm");
    ask(&mut vm, ROWS);
    vm.frame(0).expect("frame");
    assert_eq!(vm.world.pool_len[0], ROWS);
    assert!(vm.take_overruns().is_empty());
}
