//! Assets as the GPU wants them: an OBJ becomes a vertex list, a PNG becomes
//! RGBA pixels. Decoding is pure CPU; `Gpu::set_resource` uploads the result.

use anyhow::{bail, Context, Result};
use image::ImageFormat;

/// Triangle list; each vertex is position xyz then normal xyz.
#[derive(Debug)]
pub struct MeshData {
    pub vertices: Vec<[f32; 6]>,
}

#[derive(Debug)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub enum Resource {
    Mesh(MeshData),
    Texture(ImageData),
}

pub(crate) struct GpuMesh {
    pub buffer: wgpu::Buffer,
    pub vertices: u32,
}

pub(crate) struct GpuTexture {
    pub view: wgpu::TextureView,
}

pub(crate) enum Loaded {
    Mesh(GpuMesh),
    Texture(GpuTexture),
}

fn floats<const N: usize>(parts: &[&str], line: usize) -> Result<[f32; N]> {
    if parts.len() < N {
        bail!("obj line {line}: expected {N} numbers");
    }
    let mut out = [0.0; N];
    for (o, p) in out.iter_mut().zip(parts) {
        *o = p.parse().with_context(|| format!("obj line {line}: bad number `{p}`"))?;
    }
    Ok(out)
}

/// Resolve a 1-based (or negative, from the end) index into `len` items.
fn index(s: &str, len: usize, line: usize) -> Result<usize> {
    let i: i64 = s.parse().with_context(|| format!("obj line {line}: bad index `{s}`"))?;
    let r = if i > 0 { i - 1 } else { len as i64 + i };
    if i == 0 || r < 0 || r >= len as i64 {
        bail!("obj line {line}: index {i} is out of range");
    }
    Ok(r as usize)
}

fn flat_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l == 0.0 {
        [0.0, 0.0, 1.0]
    } else {
        [n[0] / l, n[1] / l, n[2] / l]
    }
}

pub fn parse_obj(text: &str) -> Result<MeshData> {
    let (mut pos, mut nor) = (Vec::<[f32; 3]>::new(), Vec::<[f32; 3]>::new());
    let mut vertices = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let mut it = raw.split_whitespace();
        let parts: Vec<&str> = it.by_ref().collect();
        match parts.first().copied() {
            Some("v") => pos.push(floats(&parts[1..], line)?),
            Some("vn") => nor.push(floats(&parts[1..], line)?),
            Some("f") => {
                if parts.len() < 4 {
                    bail!("obj line {line}: a face needs at least 3 vertices");
                }
                let mut corners = Vec::new();
                for p in &parts[1..] {
                    let mut f = p.split('/');
                    let v = index(f.next().unwrap_or(""), pos.len(), line)?;
                    let _vt = f.next();
                    let vn = match f.next() {
                        Some(s) if !s.is_empty() => Some(index(s, nor.len(), line)?),
                        _ => None,
                    };
                    corners.push((v, vn));
                }
                for i in 1..corners.len() - 1 {
                    let tri = [corners[0], corners[i], corners[i + 1]];
                    let flat = flat_normal(pos[tri[0].0], pos[tri[1].0], pos[tri[2].0]);
                    for (v, vn) in tri {
                        let p = pos[v];
                        let nn = vn.map_or(flat, |k| nor[k]);
                        vertices.push([p[0], p[1], p[2], nn[0], nn[1], nn[2]]);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(MeshData { vertices })
}

pub fn decode_png(bytes: &[u8]) -> Result<ImageData> {
    let img = image::load_from_memory_with_format(bytes, ImageFormat::Png)?.to_rgba8();
    Ok(ImageData { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

/// `kind` is the asset's extension; anything but `obj` and `png` is a blob.
pub fn decode(kind: &str, bytes: &[u8]) -> Result<Option<Resource>> {
    Ok(match kind {
        "obj" => {
            let text = std::str::from_utf8(bytes).context("obj is not utf-8")?;
            Some(Resource::Mesh(parse_obj(text)?))
        }
        "png" => Some(Resource::Texture(decode_png(bytes)?)),
        _ => None,
    })
}

pub(crate) fn upload_texture(device: &wgpu::Device, queue: &wgpu::Queue, img: &ImageData) -> GpuTexture {
    use wgpu::util::DeviceExt;
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("asset texture"),
            size: wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &img.rgba,
    );
    GpuTexture { view: texture.create_view(&Default::default()) }
}

pub(crate) fn upload_mesh(device: &wgpu::Device, mesh: &MeshData) -> GpuMesh {
    use wgpu::util::DeviceExt;
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("asset mesh"),
        contents: bytemuck_cast(&mesh.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    GpuMesh { buffer, vertices: mesh.vertices.len() as u32 }
}

fn bytemuck_cast(v: &[[f32; 6]]) -> &[u8] {
    // SAFETY: [f32; 6] has no padding and every bit pattern is a valid u8.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}
