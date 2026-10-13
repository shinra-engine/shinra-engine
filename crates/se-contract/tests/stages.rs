//! `se.stages` v2: kind and group survive a roundtrip; other versions do not.

use se_contract::stages::{decode, encode, PAIR, READ, ROW, WRITE};

#[test]
fn pair_entry_roundtrips() {
    let bytes = encode("meet", PAIR, &[("orbit", WRITE, 0), ("transform", READ, 0), ("orbit", WRITE, 1)]);
    let s = decode(&bytes).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!((s[0].name.as_str(), s[0].kind), ("meet", PAIR));
    let g: Vec<(&str, u32, u32)> =
        s[0].params.iter().map(|p| (p.component.as_str(), p.access, p.group)).collect();
    assert_eq!(g, [("orbit", WRITE, 0), ("transform", READ, 0), ("orbit", WRITE, 1)]);
}

#[test]
fn row_entry_is_kind_row_and_old_version_is_refused() {
    let mut bytes = encode("s", ROW, &[("transform", WRITE, 0)]);
    assert_eq!(decode(&bytes).unwrap()[0].kind, ROW);
    bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
    let e = decode(&bytes).unwrap_err().to_string();
    assert!(e.contains("rebuild the module"), "{e}");
}
