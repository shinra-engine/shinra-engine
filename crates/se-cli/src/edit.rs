//! Operator text back to field bytes: the inverse of `inspect::scalar`.

use anyhow::{anyhow, bail, Result};
use crate::inspect::Target;
use se_contract::layout::Ty;
use se_vm::Vm;

fn name(ty: Ty) -> String {
    match ty {
        Ty::F32 => "f32".into(),
        Ty::I32 => "i32".into(),
        Ty::U32 => "u32".into(),
        Ty::Vec(n) => format!("vec{n}"),
    }
}

fn float(ty: Ty, word: &str) -> Result<f32> {
    let v: f32 = word
        .parse()
        .map_err(|_| anyhow!("expected {}, got {:?}", name(ty), word))?;
    if !v.is_finite() {
        bail!("expected a finite {}, got {:?}", name(ty), word);
    }
    Ok(v)
}

/// Exactly `ty.size()` little-endian bytes for `text`, or an error that names
/// the type and what was typed.
pub fn encode(ty: Ty, text: &str) -> Result<Vec<u8>> {
    let text = text.trim();
    let bad = || anyhow!("expected {}, got {:?}", name(ty), text);
    let bytes = match ty {
        Ty::F32 => float(ty, text)?.to_le_bytes().to_vec(),
        Ty::I32 => text.parse::<i32>().map_err(|_| bad())?.to_le_bytes().to_vec(),
        Ty::U32 => text.parse::<u32>().map_err(|_| bad())?.to_le_bytes().to_vec(),
        Ty::Vec(n) => {
            let words: Vec<&str> = text
                .split(|c: char| c == ',' || c == '[' || c == ']' || c.is_whitespace())
                .filter(|w| !w.is_empty())
                .collect();
            if words.len() != n as usize {
                bail!("expected {n} numbers for {}, got {}", name(ty), words.len());
            }
            let mut out = Vec::with_capacity(ty.size() as usize);
            for w in words {
                out.extend(float(ty, w)?.to_le_bytes());
            }
            out
        }
    };
    debug_assert_eq!(bytes.len(), ty.size() as usize);
    Ok(bytes)
}

/// Encode `text` and write it over the target field, and nothing else. Every
/// check comes before the write, so a failure leaves the arena as it was.
pub fn commit(vm: &mut Vm, target: &Target, text: &str) -> Result<()> {
    let rec = vm
        .meta
        .components
        .get(target.col as usize)
        .ok_or_else(|| anyhow!("no column {}", target.col))?;
    let f = rec
        .fields
        .get(target.field)
        .ok_or_else(|| anyhow!("{} has no field {}", rec.name, target.field))?;
    if f.offset as u64 + f.ty.size() as u64 > rec.size as u64 {
        bail!("{}.{} does not fit in {} bytes", rec.name, f.name, rec.size);
    }
    let (ty, offset) = (f.ty, f.offset);
    let bytes = encode(ty, text)?;
    let at = vm.world.row_at(target.col, target.row) + offset;
    vm.write(at, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars() {
        assert_eq!(encode(Ty::F32, "1.5").unwrap(), 1.5f32.to_le_bytes());
        assert_eq!(encode(Ty::I32, "-3").unwrap(), (-3i32).to_le_bytes());
        assert_eq!(encode(Ty::U32, "7").unwrap(), 7u32.to_le_bytes());
    }

    #[test]
    fn vectors_in_every_spelling() {
        let want: Vec<u8> = [1f32, 2.0, 3.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        for t in ["[1, 2, 3]", "1 2 3", "1,2,3"] {
            assert_eq!(encode(Ty::Vec(3), t).unwrap(), want, "{t}");
        }
    }

    #[test]
    fn wrong_count_names_it() {
        let e = encode(Ty::Vec(3), "1 2").unwrap_err().to_string();
        assert_eq!(e, "expected 3 numbers for vec3, got 2");
    }

    #[test]
    fn junk_is_an_error() {
        let huge = "9".repeat(10_000);
        for ty in [Ty::F32, Ty::I32, Ty::U32, Ty::Vec(3)] {
            for t in ["", "abc", "nan", "inf", "-inf", "[nan, 1, 2]", "1.5", "-1", huge.as_str()] {
                let r = encode(ty, t);
                if let Ok(b) = r {
                    assert_eq!(b.len(), ty.size() as usize);
                }
            }
        }
        assert!(encode(Ty::F32, "nan").is_err());
        assert!(encode(Ty::F32, "").is_err());
        assert!(encode(Ty::U32, "-1").is_err());
        assert!(encode(Ty::I32, "1.5").is_err());
        assert!(encode(Ty::Vec(3), "[nan, 1, 2]").is_err());
    }

    fn game1() -> Vm {
        let wit = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../se-contract/tests/game1.wit");
        let g = se_contract::generate(&wit).unwrap();
        let mut vm = Vm::new(&g.module, 8).unwrap();
        vm.set_len(0, 2).unwrap();
        vm
    }

    fn pos(vm: &Vm, row: u32) -> Target {
        let col = vm.meta.components.iter().position(|r| r.name == "transform").unwrap() as u32;
        let field = vm.meta.components[col as usize]
            .fields
            .iter()
            .position(|f| f.name == "pos")
            .unwrap();
        Target { col, row, field }
    }

    #[test]
    fn commit_changes_only_the_field() {
        let mut vm = game1();
        let t = pos(&vm, 1);
        let before = vm.snapshot();
        commit(&mut vm, &t, "[1, 2, 3]").unwrap();
        let at = vm.world.row_at(t.col, 1);
        let got = vm.read(at, 12).unwrap();
        let want: Vec<u8> = [1f32, 2.0, 3.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        assert_eq!(got, want);
        let after = vm.snapshot();
        let base = vm.meta.base;
        let lo = (at - base) as usize;
        for (i, (a, b)) in before.iter().zip(&after).enumerate() {
            if !(lo..lo + 12).contains(&i) {
                assert_eq!(a, b, "byte {i} moved");
            }
        }
        assert_ne!(before, after);
    }

    #[test]
    fn bad_text_leaves_memory_alone() {
        let mut vm = game1();
        let t = pos(&vm, 1);
        let before = vm.snapshot();
        assert!(commit(&mut vm, &t, "oops").is_err());
        assert_eq!(vm.snapshot(), before);
    }

    #[test]
    fn row_zero_does_not_touch_row_one() {
        let mut vm = game1();
        let t1 = pos(&vm, 1);
        let at1 = vm.world.row_at(t1.col, 1);
        let before = vm.read(at1, 12).unwrap();
        let t0 = pos(&vm, 0);
        commit(&mut vm, &t0, "4 5 6").unwrap();
        assert_eq!(vm.read(at1, 12).unwrap(), before);
    }

    #[test]
    fn edit_round_trip_changes_only_the_field() {
        use crate::inspect::inspect;
        let mut vm = game1();
        let sel = 2; // hierarchy: heading, bodies[0], bodies[1]
        let t = pos(&vm, 1);
        let at = vm.world.row_at(t.col, 1)
            + vm.meta.components[t.col as usize].fields[t.field].offset;
        // Seed the field with a byte no encoded float contains, so every byte
        // of the edit shows up in the diff.
        vm.write(at, &[0xAA; 12]).unwrap();

        let (_, map) = inspect(&vm, sel);
        let i = map.iter().position(|m| *m == Some(t)).expect("pos row has a Target");
        let target = map[i].unwrap();

        let before = vm.snapshot();
        commit(&mut vm, &target, "[1, 2, 3]").unwrap();
        let after = vm.snapshot();

        let changed: Vec<usize> = before
            .iter()
            .zip(&after)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        let lo = (at - vm.meta.base) as usize;
        assert_eq!(changed, (lo..lo + 12).collect::<Vec<_>>());

        let (rows, _) = inspect(&vm, sel);
        let shown = rows.iter().find(|(k, _)| k == "  pos").expect("a pos row");
        assert_eq!(shown.1, "[1.000, 2.000, 3.000]");
    }

    #[test]
    fn a_bad_field_index_is_an_error() {
        let mut vm = game1();
        let mut t = pos(&vm, 0);
        t.field = 99;
        let before = vm.snapshot();
        assert!(commit(&mut vm, &t, "1").is_err());
        assert_eq!(vm.snapshot(), before);
    }
}
