//! Snapshots, exercised against the sparse-set store.
//!
//! `se-host` cannot host these tests: it defines `Store` but deliberately
//! depends on no implementation of it. That inversion is the point being
//! tested as much as the file format is — everything below reaches the data
//! only through `World`\'s public surface, so a snapshot written by one store
//! is readable by any other that answers the same contract.

use se_ecs::World;
use se_host::registry::LayoutDef;
use se_host::snapshot;

fn layout(name: &str, size: u32, hash: u64) -> LayoutDef {
    LayoutDef { name: name.into(), size, align: 4, fields: Vec::new(), hash }
}

fn seeded() -> World {
    let mut w = World::new();
    w.declare(&[layout("Pos", 8, 0xA), layout("Tag", 4, 0xB)]);
    for i in 0..4u32 {
        let e = w.spawn();
        w.set(e, "Pos", &[i as u8; 8]);
        // Only the even ones carry a Tag, so the round trip has to preserve
        // which entities a component is *absent* from, not just its bytes.
        if i % 2 == 0 {
            w.set(e, "Tag", &i.to_le_bytes());
        }
    }
    w
}

fn dump(w: &World) -> Vec<(usize, Option<Vec<u8>>, Option<Vec<u8>>)> {
    w.entities()
        .iter()
        .enumerate()
        .map(|(i, &e)| {
            (i, w.get(e, "Pos").map(<[u8]>::to_vec), w.get(e, "Tag").map(<[u8]>::to_vec))
        })
        .collect()
}

#[test]
fn round_trip_preserves_components_and_absences() {
    let dir = std::env::temp_dir().join("se_snap_rt");
    let path = dir.join("s.sesnap");
    let src = seeded();
    snapshot::save(&src, 0x1234, &path).unwrap();

    let mut dst = World::new();
    dst.declare(&[layout("Pos", 8, 0xA), layout("Tag", 4, 0xB)]);
    snapshot::load(&mut dst, 0x1234, &path).unwrap();

    assert_eq!(dst.entities().len(), 4);
    assert_eq!(dump(&dst), dump(&src));
    std::fs::remove_file(&path).ok();
}

#[test]
fn load_replaces_rather_than_merges() {
    let dir = std::env::temp_dir().join("se_snap_rep");
    let path = dir.join("s.sesnap");
    snapshot::save(&seeded(), 7, &path).unwrap();

    let mut dst = seeded();
    for _ in 0..3 {
        let e = dst.spawn();
        dst.set(e, "Pos", &[0xFF; 8]);
    }
    assert_eq!(dst.entities().len(), 7);
    snapshot::load(&mut dst, 7, &path).unwrap();
    assert_eq!(dst.entities().len(), 4, "load must not leave the old entities behind");
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_different_contract_is_refused() {
    let dir = std::env::temp_dir().join("se_snap_ctr");
    let path = dir.join("s.sesnap");
    snapshot::save(&seeded(), 1, &path).unwrap();

    let mut dst = seeded();
    let err = snapshot::load(&mut dst, 2, &path).unwrap_err().to_string();
    assert!(err.contains("load the matching game set"), "{err}");
    assert_eq!(dst.entities().len(), 4, "a refused load must not disturb the store");
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_component_that_changed_shape_is_refused() {
    let dir = std::env::temp_dir().join("se_snap_shape");
    let path = dir.join("s.sesnap");
    snapshot::save(&seeded(), 9, &path).unwrap();

    // Same bundle hash, but `Pos` was rebuilt to a different layout.
    let mut dst = World::new();
    dst.declare(&[layout("Pos", 8, 0xBEEF), layout("Tag", 4, 0xB)]);
    let err = snapshot::load(&mut dst, 9, &path).unwrap_err().to_string();
    assert!(err.contains("different shape"), "{err}");
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_truncated_file_leaves_the_store_alone() {
    let dir = std::env::temp_dir().join("se_snap_trunc");
    let path = dir.join("s.sesnap");
    snapshot::save(&seeded(), 3, &path).unwrap();
    let full = std::fs::read(&path).unwrap();
    std::fs::write(&path, &full[..full.len() - 6]).unwrap();

    let mut dst = seeded();
    let before = dump(&dst);
    assert!(snapshot::load(&mut dst, 3, &path).is_err());
    assert_eq!(dump(&dst), before, "a bad file must not half-replace the store");
    std::fs::remove_file(&path).ok();
}

