//! Images an `asset/*.so` published, as GPU textures.
//!
//! `asset/*.so` hands over bytes and says nothing about what they mean; the
//! render side decides. A name ending `.png` is an image, the same way one
//! ending `.obj` is a mesh — so an asset module can change what a sprite sheet
//! *is* without the engine learning a new asset type, which is the whole point
//! of the asset slot.

use anyhow::{bail, Context, Result};
use wgpu::{Device, Queue, Texture, TextureView};

pub struct ImageTexture {
    pub view: TextureView,
    pub size: (u32, u32),
    _texture: Texture,
}

/// The identifier a shader sees: the asset name without its extension, with
/// anything that is not a WGSL identifier character folded to `_`.
///
/// `"sprites.png"` becomes `sprites`, so a pass that declared it can write
/// `textureSample(sprites, se_sampler, uv)` and never repeat the file name.
pub fn shader_name(asset: &str) -> String {
    let stem = asset.rsplit('/').next().unwrap_or(asset);
    let stem = stem.split('.').next().unwrap_or(stem);
    let mut out: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    // A WGSL identifier cannot start with a digit.
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if out.is_empty() {
        out.push_str("tex");
    }
    out
}

pub fn load(device: &Device, queue: &Queue, name: &str, bytes: Option<&[u8]>) -> Result<ImageTexture> {
    let Some(bytes) = bytes else {
        bail!("no asset named `{name}` — is the asset module in the bundle?")
    };
    if !name.to_ascii_lowercase().ends_with(".png") {
        bail!("`{name}` is not an image this build can read (expected .png)");
    }

    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .with_context(|| format!("decoding `{name}`"))?
        .to_rgba8();
    let (w, h) = (img.width().max(1), img.height().max(1));

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(name),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Not sRGB: an atlas is usually authored in sRGB, but the passes here
        // write to Rgba8Unorm targets and do their own shading, so keeping it
        // linear means what the artist drew is what lands in the buffer.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &img,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * w),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(ImageTexture { view, size: (w, h), _texture: texture })
}
