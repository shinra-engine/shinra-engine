use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

impl Vertex {
    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Load a single combined mesh from an OBJ file. Multi-group OBJs are
    /// flattened — MVP doesn't need per-group separation.
    pub fn from_obj_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let (models, _) = tobj::load_obj(
            path.as_ref(),
            &tobj::LoadOptions {
                triangulate: true,
                ignore_points: true,
                ignore_lines: true,
                ..Default::default()
            },
        )?;

        let mut vertices: Vec<Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();

        for model in &models {
            let mesh = &model.mesh;
            let index_offset = vertices.len() as u32;

            let pos_count = mesh.positions.len() / 3;
            let has_normals = mesh.normals.len() == mesh.positions.len();

            if has_normals {
                for i in 0..pos_count {
                    vertices.push(Vertex {
                        position: [
                            mesh.positions[3 * i],
                            mesh.positions[3 * i + 1],
                            mesh.positions[3 * i + 2],
                        ],
                        normal: [
                            mesh.normals[3 * i],
                            mesh.normals[3 * i + 1],
                            mesh.normals[3 * i + 2],
                        ],
                    });
                }
                for &idx in &mesh.indices {
                    indices.push(index_offset + idx);
                }
            } else {
                // No normals: expand to unindexed triangles, compute flat normals per face.
                let tri_count = mesh.indices.len() / 3;
                for tri in 0..tri_count {
                    let i0 = mesh.indices[3 * tri] as usize;
                    let i1 = mesh.indices[3 * tri + 1] as usize;
                    let i2 = mesh.indices[3 * tri + 2] as usize;

                    let p0 = glam::Vec3::from_slice(&mesh.positions[3 * i0..3 * i0 + 3]);
                    let p1 = glam::Vec3::from_slice(&mesh.positions[3 * i1..3 * i1 + 3]);
                    let p2 = glam::Vec3::from_slice(&mesh.positions[3 * i2..3 * i2 + 3]);

                    let normal = (p1 - p0).cross(p2 - p0).normalize_or_zero();
                    let n = normal.to_array();

                    let base = vertices.len() as u32;
                    vertices.push(Vertex {
                        position: p0.to_array(),
                        normal: n,
                    });
                    vertices.push(Vertex {
                        position: p1.to_array(),
                        normal: n,
                    });
                    vertices.push(Vertex {
                        position: p2.to_array(),
                        normal: n,
                    });
                    indices.push(base);
                    indices.push(base + 1);
                    indices.push(base + 2);
                }
            }
        }

        Ok(Mesh { vertices, indices })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_mesh(path: &str) {
        let mesh = Mesh::from_obj_file(path).expect("load failed");
        assert!(!mesh.vertices.is_empty(), "no vertices");
        assert!(!mesh.indices.is_empty(), "no indices");
        let max_idx = *mesh.indices.iter().max().unwrap() as usize;
        assert!(max_idx < mesh.vertices.len(), "index out of range");
        for v in &mesh.vertices {
            let len =
                (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1] + v.normal[2] * v.normal[2])
                    .sqrt();
            assert!(
                (len - 1.0).abs() < 1e-3,
                "normal not unit length: {len} for {:?}",
                v.normal
            );
        }
    }

    #[test]
    fn teapot_loads() {
        check_mesh("../assets/teapot.obj");
    }

    #[test]
    fn bunny_loads() {
        check_mesh("../assets/bunny.obj");
    }
}
