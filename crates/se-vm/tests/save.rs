//! Save/load needs no wasm module: only a contract and a memory.

use se_vm::Vm;
use std::path::PathBuf;

fn wit() -> String {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit"))
        .unwrap()
}

fn contract(text: &str, name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&p, text).unwrap();
    se_contract::generate(&p).unwrap().module
}

fn vm() -> Vm {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    Vm::new(&se_contract::generate(&p).unwrap().module, 64).unwrap()
}

/// Seed a control var, a column row and a blob, then return the camera offset.
fn seed(vm: &mut Vm) -> u32 {
    let (cam, _) = vm.meta.control_at("camera").unwrap();
    vm.write(cam, &[7, 7, 7, 7]).unwrap();
    let col = vm.world.row_at(0, 3);
    vm.write(col, &[9, 8, 7, 6]).unwrap();
    vm.put_blob(0, &[1, 2, 3, 4, 5]).unwrap();
    cam
}

#[test]
fn roundtrip() {
    let mut vm = vm();
    let cam = seed(&mut vm);
    let before = vm.snapshot();
    let file = vm.save(42);
    vm.write(cam, &[0, 0, 0, 0]).unwrap();
    vm.write(vm.world.row_at(0, 3), &[0; 4]).unwrap();
    assert_eq!(vm.load_save(&file).unwrap(), 42);
    assert_eq!(vm.snapshot(), before);
}

#[test]
fn other_game() {
    let mut a = vm();
    seed(&mut a);
    let file = a.save(1);
    let other = wit().replace("record intent { swipe: s32, quit: u32 }", "record intent { swipe: s32, quit: u32, extra: u32 }");
    assert_ne!(other, wit());
    let mut b = Vm::new(&contract(&other, "other.wit"), 64).unwrap();
    let before = b.bytes().to_vec();
    let e = b.load_save(&file).unwrap_err().to_string();
    assert!(e.contains("contract differs"), "{e}");
    assert_eq!(b.bytes(), &before[..]);
}

#[test]
fn corrupt() {
    let mut vm = vm();
    seed(&mut vm);
    let good = vm.save(1);
    let before = vm.bytes().to_vec();
    let mut bad_magic = good.clone();
    bad_magic[0] = b'X';
    let mut v2 = good.clone();
    v2[4] = 2;
    let mut base = good.clone();
    base[20] ^= 0x10;
    let short = good[..good.len() - 1].to_vec();
    for f in [bad_magic, v2, base, short] {
        assert!(vm.load_save(&f).is_err());
    }
    assert_eq!(vm.bytes(), &before[..]);
}

#[test]
fn mirrors() {
    let mut vm = vm();
    seed(&mut vm);
    vm.set_len(0, 5).unwrap();
    let (top, len) = (vm.world.blob_top, vm.world.pool_len.clone());
    let file = vm.save(0);
    vm.put_blob(1, &[0; 64]).unwrap();
    vm.set_len(0, 2).unwrap();
    vm.load_save(&file).unwrap();
    assert_eq!(vm.world.blob_top, top);
    assert_eq!(vm.world.pool_len, len);
}
