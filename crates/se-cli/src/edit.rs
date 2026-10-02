//! Operator text back to field bytes: the inverse of `inspect::scalar`.

use anyhow::{anyhow, bail, Result};
use se_contract::layout::Ty;

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
}
