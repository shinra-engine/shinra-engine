//! Where `Vm::take_sound()`'s channel ends: resolve the sample bytes from the
//! asset blob the sound named, then hand them to whichever `AudioSink` the
//! host chose.
//!
//! No audio crate exists yet (verified: no rodio/cpal/kira in Cargo.lock, and
//! this WSL2 host's `/dev/snd` has only `timer`) -- see story-66's Open
//! Decision. `HeadlessSink` below is the only sink until a real device one
//! lands; it keeps `shinra run`/`shinra shot` deterministic in the meantime.

use se_vm::sound::SoundEvent;
use se_vm::Vm;

/// Where a resolved sound request goes once its asset bytes are in hand.
pub trait AudioSink {
    fn play(&mut self, bytes: &[u8], gain: f32);
}

/// Logs what would have played instead of touching a device.
#[derive(Default)]
pub struct HeadlessSink {
    log: Vec<String>,
}

impl HeadlessSink {
    /// Drain what played since the last call -- same shape as
    /// `Vm::take_overruns`, so a caller logs it the same way.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }
}

impl AudioSink for HeadlessSink {
    fn play(&mut self, bytes: &[u8], gain: f32) {
        self.log
            .push(format!("sound: {} bytes, gain {gain:.3}", bytes.len()));
    }
}

/// Resolve `event`'s asset slot and hand its bytes to `sink`. A missing or
/// out-of-range slot comes back as a warning string, never a panic -- the
/// game asked for a sound that doesn't exist, which is a content bug, not an
/// engine fault.
pub fn dispatch(vm: &Vm, event: SoundEvent, sink: &mut dyn AudioSink) -> Result<(), String> {
    match vm.blob(event.id) {
        Ok(bytes) => {
            sink.play(&bytes, event.gain);
            Ok(())
        }
        Err(e) => Err(format!("sound: asset slot {} unavailable: {e:#}", event.id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn vm() -> Vm {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
        Vm::new(&se_contract::generate(&p).unwrap().module, 64).unwrap()
    }

    #[test]
    fn resolves_a_valid_slot_into_the_sink() {
        let mut vm = vm();
        vm.put_blob(3, &[9, 8, 7]).unwrap();
        let mut sink = HeadlessSink::default();

        dispatch(&vm, SoundEvent { id: 3, gain: 0.5 }, &mut sink).expect("slot 3 is loaded");

        let log = sink.take_log();
        assert_eq!(log.len(), 1);
        assert!(log[0].contains("3 bytes"));
        assert!(log[0].contains("0.500"));
    }

    #[test]
    fn an_out_of_range_slot_warns_instead_of_panicking() {
        let vm = vm();
        let mut sink = HeadlessSink::default();

        let err = dispatch(&vm, SoundEvent { id: 999, gain: 1.0 }, &mut sink)
            .expect_err("slot 999 is past the contract's blob_slots");

        assert!(err.contains("999"));
        assert!(sink.take_log().is_empty(), "no sink call on a missing slot");
    }
}
