//! `take_sound` edge-detects `control.sound.seq`. No wasm module needed, only
//! a contract and a memory — same shape as `save.rs`'s tests.

use se_vm::Vm;
use std::path::PathBuf;

fn vm() -> Vm {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    Vm::new(&se_contract::generate(&p).unwrap().module, 64).unwrap()
}

fn write_sound(vm: &mut Vm, seq: u32, id: u32, gain: f32) {
    let (at, _) = vm.meta.control_at("sound").expect("sound");
    let mut b = [0u8; 12];
    b[0..4].copy_from_slice(&seq.to_le_bytes());
    b[4..8].copy_from_slice(&id.to_le_bytes());
    b[8..12].copy_from_slice(&gain.to_le_bytes());
    vm.write(at, &b).unwrap();
}

#[test]
fn fires_once_on_a_seq_change_then_stays_silent() {
    let mut vm = vm();
    assert_eq!(
        vm.take_sound().unwrap(),
        None,
        "a fresh vm's seq is 0, the baseline"
    );

    write_sound(&mut vm, 1, 7, 0.5);
    let e = vm.take_sound().unwrap().expect("seq changed 0 -> 1");
    assert_eq!((e.id, e.gain), (7, 0.5));

    assert_eq!(
        vm.take_sound().unwrap(),
        None,
        "no further write, no re-trigger"
    );
}

#[test]
fn load_save_leaves_edge_detection_to_the_next_take_sound() {
    let mut vm = vm();
    write_sound(&mut vm, 3, 1, 1.0);
    vm.take_sound().unwrap(); // last_sound_seq is now 3
    let snapshot = vm.save(0); // the snapshot's sound.seq is also 3

    write_sound(&mut vm, 9, 2, 1.0);
    vm.take_sound().unwrap(); // last_sound_seq is now 9

    // Restoring a snapshot whose seq (3) differs from what this Vm last saw
    // (9) fires exactly once.
    vm.load_save(&snapshot).unwrap();
    let e = vm
        .take_sound()
        .unwrap()
        .expect("restored seq (3) differs from last-seen (9)");
    assert_eq!(e.id, 1);
    assert_eq!(
        vm.take_sound().unwrap(),
        None,
        "no re-trigger on a repeat call"
    );

    // Restoring a snapshot whose seq (3) already equals what this Vm just
    // reported (3, re-established above) does not fire.
    vm.load_save(&snapshot).unwrap();
    assert_eq!(
        vm.take_sound().unwrap(),
        None,
        "restored seq equals last-seen; stays silent"
    );
}
