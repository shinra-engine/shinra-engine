//! Blob slots written at run time: no module is loaded, only the contract.

use se_vm::Vm;
use std::path::PathBuf;

fn vm() -> Vm {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/spans.wit");
    let g = se_contract::generate(&wit).expect("spans.wit generates");
    Vm::new(&g.module, 16).expect("vm")
}

fn span(vm: &Vm, slot: u32) -> (u32, u32) {
    let b = vm.read(vm.meta.blobs_at + slot * 8, 8).unwrap();
    (
        u32::from_le_bytes(b[..4].try_into().unwrap()),
        u32::from_le_bytes(b[4..].try_into().unwrap()),
    )
}

#[test]
fn roundtrip() {
    let mut vm = vm();
    vm.put_str(0, "hello").unwrap();
    assert_eq!(vm.blob(0).unwrap(), b"hello");
    vm.put_u32s(1, &[1, 0x0102_0304]).unwrap();
    assert_eq!(vm.blob(1).unwrap(), [1, 0, 0, 0, 4, 3, 2, 1]);
    vm.put_f32s(2, &[1.0]).unwrap();
    assert_eq!(vm.blob(2).unwrap(), 1.0f32.to_le_bytes());
}

#[test]
fn reuse() {
    let mut vm = vm();
    vm.put_str(0, "abcdef").unwrap();
    let (off, _) = span(&vm, 0);
    let top = vm.world.blob_top;
    vm.put_str(0, "xyz").unwrap();
    assert_eq!(vm.world.blob_top, top);
    assert_eq!(span(&vm, 0), (off, 3));
    assert_eq!(vm.blob(0).unwrap(), b"xyz");
    vm.put_str(0, "uvwxyz").unwrap();
    assert_eq!(vm.world.blob_top, top);
    vm.put_str(0, "a much longer payload").unwrap();
    assert!(vm.world.blob_top > top);
    assert_ne!(span(&vm, 0).0, off);
    assert_eq!(vm.blob(0).unwrap(), b"a much longer payload");
}

#[test]
fn slot_range() {
    let mut vm = vm();
    let e = vm.put_str(64, "x").unwrap_err().to_string();
    assert!(e.contains("blob slot 64 is past the 64"), "{e}");
    assert!(vm.blob(64).is_err());
}

#[test]
fn field() {
    let mut vm = vm();
    vm.put_str(3, "span me").unwrap();
    let at = vm.meta.heap - 64;
    vm.set_span_field(at, 3).unwrap();
    let (off, _) = span(&vm, 3);
    let mut want = off.to_le_bytes().to_vec();
    want.extend(7u32.to_le_bytes());
    assert_eq!(vm.read(at, 8).unwrap(), want);
}
