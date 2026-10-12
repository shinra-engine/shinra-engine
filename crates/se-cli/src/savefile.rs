//! Quick save and quick load: `target/shinra/<game>/quick.save`.
//!
//! The file is the VM's save format (header + arena). It holds everything the
//! world owns and nothing a module owns: the wasm stack and module statics are
//! not saved, because state that must survive lives in the arena.

use anyhow::{bail, Context, Result};
use se_vm::Vm;
use std::path::{Path, PathBuf};

pub fn path(game: &str) -> PathBuf {
    Path::new("target/shinra").join(game).join("quick.save")
}

pub fn save(vm: &Vm, game: &str, frame: u64) -> Result<String> {
    write_to(vm, &path(game), frame)
}

pub fn load(vm: &mut Vm, game: &str) -> Result<(u64, String)> {
    read_from(vm, &path(game), game)
}

pub fn write_to(vm: &Vm, file: &Path, frame: u64) -> Result<String> {
    let bytes = vm.save(frame as u32);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let tmp = file.with_extension("save.tmp");
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, file).with_context(|| format!("rename to {}", file.display()))?;
    Ok(format!("saved {} B at frame {frame}", bytes.len()))
}

pub fn read_from(vm: &mut Vm, file: &Path, game: &str) -> Result<(u64, String)> {
    if !file.is_file() {
        bail!("no save for {game} yet")
    }
    let bytes = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
    let frame = vm.load_save(&bytes)? as u64;
    Ok((frame, format!("loaded {} B, resuming at frame {frame}", bytes.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm() -> Vm {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../se-contract/tests/game1.wit");
        Vm::new(&se_contract::generate(&p).unwrap().module, 64).unwrap()
    }

    fn dir(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/savefile-test").join(name);
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn roundtrip_restores_bytes_and_frame() {
        let f = dir("roundtrip").join("quick.save");
        let mut a = vm();
        a.put_blob(0, &[1, 2, 3, 4]).unwrap();
        let before = a.snapshot();
        let note = write_to(&a, &f, 42).unwrap();
        assert!(note.contains("at frame 42"));
        assert!(!f.with_extension("save.tmp").exists());

        let mut b = vm();
        let (frame, _) = read_from(&mut b, &f, "g").unwrap();
        assert_eq!(frame, 42);
        assert_eq!(b.snapshot(), before);
    }

    #[test]
    fn missing_file_names_the_game() {
        let f = dir("missing").join("quick.save");
        let e = read_from(&mut vm(), &f, "game9").unwrap_err();
        assert_eq!(e.to_string(), "no save for game9 yet");
    }

    #[test]
    fn corrupt_file_leaves_memory_untouched() {
        let f = dir("corrupt").join("quick.save");
        let mut a = vm();
        write_to(&a, &f, 7).unwrap();
        let mut bytes = std::fs::read(&f).unwrap();
        bytes[0] ^= 0xff;
        std::fs::write(&f, &bytes).unwrap();
        let before = a.snapshot();
        assert!(read_from(&mut a, &f, "g").is_err());
        assert_eq!(a.snapshot(), before);
    }
}
