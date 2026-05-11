use serde::{Deserialize, Serialize};

/// Top-level type stored in `.scn.ron`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilemap: Option<Tilemap>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translation: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }
}

/// Reference to a mesh asset (e.g., `assets/bunny.obj`). Path is
/// workspace-relative — runtime resolves it the same way the existing
/// `Mesh::from_obj_file(path)` already does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshRef {
    pub path: String,
}

/// 2D tilemap. `tileset` is a path to a `.tres.ron` Tileset file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tilemap {
    pub tileset: String,
    pub tile_size: [f32; 2], // world-space size of one tile (XZ plane)
    pub cells: Vec<Cell>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
    pub tile_id: u32,
}

/// Top-level type stored in `.tres.ron` for tilesets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tileset {
    pub name: String,
    pub tiles: Vec<Tile>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub id: u32,
    pub name: String,
    /// Linear sRGB color [r, g, b], each in 0.0..=1.0. v1 has no texture
    /// support; v2 can add `atlas: Option<String>` + `uv: Option<[u32; 4]>`
    /// without breaking existing files.
    pub color: [f32; 3],
}

/// Generic component value attached to a Node. v1 supports a small fixed
/// set (PlayerControlled is the only behavior tag right now); future
/// components can extend the enum.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ComponentValue {
    PlayerControlled,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_scene() -> Scene {
        Scene {
            name: "town".into(),
            nodes: vec![
                Node {
                    name: "ground".into(),
                    transform: Transform::default(),
                    mesh: None,
                    tilemap: Some(Tilemap {
                        tileset: "tilesets/town.tres.ron".into(),
                        tile_size: [1.0, 1.0],
                        cells: vec![
                            Cell {
                                x: 0,
                                y: 0,
                                tile_id: 1,
                            },
                            Cell {
                                x: 1,
                                y: 0,
                                tile_id: 1,
                            },
                            Cell {
                                x: 2,
                                y: 0,
                                tile_id: 5,
                            },
                        ],
                    }),
                    components: vec![],
                    children: vec![],
                },
                Node {
                    name: "bunny".into(),
                    transform: Transform {
                        translation: [3.0, 0.0, 2.0],
                        ..Default::default()
                    },
                    mesh: Some(MeshRef {
                        path: "assets/bunny.obj".into(),
                    }),
                    tilemap: None,
                    components: vec![ComponentValue::PlayerControlled],
                    children: vec![],
                },
            ],
        }
    }

    fn sample_tileset() -> Tileset {
        Tileset {
            name: "town_tiles".into(),
            tiles: vec![
                Tile {
                    id: 1,
                    name: "grass".into(),
                    color: [0.4, 0.8, 0.3],
                },
                Tile {
                    id: 2,
                    name: "river".into(),
                    color: [0.2, 0.5, 0.9],
                },
                Tile {
                    id: 5,
                    name: "path".into(),
                    color: [0.7, 0.6, 0.4],
                },
            ],
        }
    }

    #[test]
    fn scene_roundtrip() {
        let s1 = sample_scene();
        let serialized = ron::ser::to_string_pretty(&s1, Default::default()).unwrap();
        let s2: Scene = ron::from_str(&serialized).unwrap();
        assert_eq!(s1, s2);
    }

    #[test]
    fn tileset_roundtrip() {
        let t1 = sample_tileset();
        let serialized = ron::ser::to_string_pretty(&t1, Default::default()).unwrap();
        let t2: Tileset = ron::from_str(&serialized).unwrap();
        assert_eq!(t1, t2);
    }

    #[test]
    fn empty_scene_roundtrip() {
        let s1 = Scene::default();
        let serialized = ron::to_string(&s1).unwrap();
        let s2: Scene = ron::from_str(&serialized).unwrap();
        assert_eq!(s1, s2);
    }

    #[test]
    fn node_with_no_optional_fields_serializes_compactly() {
        let n = Node {
            name: "empty".into(),
            ..Default::default()
        };
        let s = ron::to_string(&n).unwrap();
        // skip_serializing_if works — the `mesh`, `tilemap`, `components`,
        // `children` fields should not appear when None/empty.
        assert!(!s.contains("mesh:"), "mesh should be skipped: {s}");
        assert!(!s.contains("tilemap:"), "tilemap should be skipped: {s}");
        assert!(
            !s.contains("components:"),
            "components should be skipped: {s}"
        );
        assert!(!s.contains("children:"), "children should be skipped: {s}");
    }
}
