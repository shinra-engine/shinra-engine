//! `command::read` on frames written by hand: no adapter, no rustc.
//!
//! The reader trusts pointers a module wrote, so every way a header can lie
//! must come back as an `Err` that names the offender, never a panic.

use anyhow::Result;
use se_gpu::command::{self, Command, COMMAND_BYTES};
use se_vm::Vm;
use std::path::PathBuf;

fn vm() -> Result<Vm> {
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
    let g = se_contract::generate(&wit)?;
    Vm::new(&g.module, 8)
}

fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn encode(c: &Command) -> Vec<u8> {
    let mut b = words(&[
        c.kind, c.vertices, c.shader, c.column, c.count, c.uniform_off, c.uniform_len, 0,
    ]);
    for f in c.clear {
        b.extend_from_slice(&f.to_le_bytes());
    }
    assert_eq!(b.len() as u32, COMMAND_BYTES);
    b
}

fn header(vm: &mut Vm, commands: u32, count: u32, present: u32, epoch: u32) -> Result<()> {
    vm.write(vm.meta.frame, &words(&[commands, count, present, epoch]))
}

fn sample(i: u32) -> Command {
    Command {
        kind: i % 4,
        vertices: 3 + i,
        shader: i,
        column: 10 + i,
        count: 100 + i,
        uniform_off: 4096 + i * 64,
        uniform_len: 64 + i,
        clear: [0.25 * i as f32, 0.5, 0.75, 1.0],
    }
}

#[test]
fn commands_and_epoch_come_back_as_written() -> Result<()> {
    let mut vm = vm()?;
    let want: Vec<Command> = (0..3).map(sample).collect();
    let at = vm.meta.commands;
    for (i, c) in want.iter().enumerate() {
        vm.write(at + i as u32 * COMMAND_BYTES, &encode(c))?;
    }
    header(&mut vm, at, 3, 0, 9)?;
    let (got, epoch) = command::read(&vm.meta, vm.bytes())?;
    assert_eq!(got, want);
    assert_eq!(epoch, 9);
    Ok(())
}

#[test]
fn an_empty_list_is_empty() -> Result<()> {
    let mut vm = vm()?;
    let at = vm.meta.commands;
    header(&mut vm, at, 0, 0, 4)?;
    let (got, epoch) = command::read(&vm.meta, vm.bytes())?;
    assert!(got.is_empty());
    assert_eq!(epoch, 4);
    Ok(())
}

#[test]
fn more_than_the_contract_allows_is_refused() -> Result<()> {
    let mut vm = vm()?;
    assert_eq!(vm.meta.max_commands, 256);
    let at = vm.meta.commands;
    header(&mut vm, at, 257, 0, 1)?;
    let e = command::read(&vm.meta, vm.bytes()).unwrap_err().to_string();
    assert!(e.contains("allows 256"), "{e}");
    assert!(e.contains("257"), "{e}");
    Ok(())
}

#[test]
fn a_header_past_the_end_is_refused() -> Result<()> {
    let vm = vm()?;
    let short = &vm.bytes()[..vm.meta.frame as usize + 15];
    let e = command::read(&vm.meta, short).unwrap_err().to_string();
    assert!(e.contains("frame header is outside"), "{e}");
    Ok(())
}

#[test]
fn a_command_past_the_end_names_its_index() -> Result<()> {
    let mut vm = vm()?;
    let len = vm.bytes().len() as u32;
    // Room for command 0 but not command 1.
    header(&mut vm, len - COMMAND_BYTES - 1, 2, 0, 1)?;
    let e = command::read(&vm.meta, vm.bytes()).unwrap_err().to_string();
    assert!(e.contains("command 1 is outside the memory"), "{e}");

    header(&mut vm, len - 4, 1, 0, 1)?;
    let e = command::read(&vm.meta, vm.bytes()).unwrap_err().to_string();
    assert!(e.contains("command 0 is outside the memory"), "{e}");
    Ok(())
}

#[test]
fn a_pointer_that_would_overflow_u32_is_an_error_not_a_panic() -> Result<()> {
    let mut vm = vm()?;
    for at in [u32::MAX - 10, u32::MAX] {
        header(&mut vm, at, 1, 0, 1)?;
        let e = command::read(&vm.meta, vm.bytes()).unwrap_err().to_string();
        assert!(e.contains("command 0 is outside the memory"), "{e}");
    }
    Ok(())
}
