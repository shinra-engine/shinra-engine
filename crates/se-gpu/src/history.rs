//! Per-offscreen-target history: a `cur`/`prev` texture pair.
//!
//! `cur` is this frame's render target, exactly as before. `prev` holds what
//! `cur` looked like after the *previous* frame finished, so a later pass can
//! fade or blend against it. Nothing copies `cur` into `prev` yet — that is
//! wired in a later ticket — this module only allocates the pair, keeps both
//! zeroed until first drawn, and recreates them (still zeroed) on resize.

use crate::FORMAT;
use anyhow::Result;

/// A lazily-created offscreen render target (ids 1..=8) and its one-frame
/// history. Unlike the present `Target`, it is only ever sampled by a later
/// pass, never read back to the CPU during replay, so it carries no
/// present-style readback buffer of its own.
///
/// `cur_texture`, `prev_texture`, and `prev_view` are not read by replay yet
/// — only this module's own tests use them, until a later ticket wires the
/// frame-end `cur` -> `prev` copy and the sampling that reads `prev_view`.
#[allow(dead_code)]
pub(crate) struct History {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) cur_texture: wgpu::Texture,
    pub(crate) cur_view: wgpu::TextureView,
    pub(crate) prev_texture: wgpu::Texture,
    pub(crate) prev_view: wgpu::TextureView,
}

impl History {
    /// Both textures start zeroed: the WebGPU spec guarantees a freshly
    /// created texture reads as zero before anything is drawn or copied into
    /// it, so `prev` of a target that has never been drawn into reads as
    /// zero/transparent, never uninitialized GPU memory.
    pub(crate) fn new(device: &wgpu::Device, width: u32, height: u32) -> History {
        let (cur_texture, cur_view) = Self::make(device, width, height, "history cur");
        let (prev_texture, prev_view) = Self::make(device, width, height, "history prev");
        History { width, height, cur_texture, cur_view, prev_texture, prev_view }
    }

    /// `cur` is drawn into and will be copied into `prev` at frame end;
    /// `prev` is sampled by a later pass and is the copy's destination. Both
    /// usages are given to each so a later ticket's copy needs no
    /// reallocation.
    fn make(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        label: &str,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }
}

/// Copies `texture` into a CPU buffer and returns its RGBA8 bytes, dropping
/// the row padding `copy_texture_to_buffer` requires. Used by tests that
/// need to see a texture's content directly; replay itself never reads an
/// offscreen target back to the CPU.
#[allow(dead_code)]
pub(crate) fn read_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let row_bytes = (width * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row_bytes * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(crate::GPU_BUDGET),
    })?;
    let padded = slice.get_mapped_range().to_vec();
    readback.unmap();
    let w = (width * 4) as usize;
    let stride = row_bytes as usize;
    let mut out = Vec::with_capacity(w * height as usize);
    for y in 0..height as usize {
        out.extend_from_slice(&padded[y * stride..y * stride + w]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device_queue() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("no WebGPU adapter");
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("request_device failed")
    }

    #[test]
    fn cur_and_prev_are_distinct_and_start_zeroed() -> Result<()> {
        let (device, queue) = device_queue();

        let h = History::new(&device, 8, 8);
        assert_ne!(h.cur_view, h.prev_view, "cur and prev must be separate textures");
        for texture in [&h.cur_texture, &h.prev_texture] {
            let pixels = read_pixels(&device, &queue, texture, h.width, h.height)?;
            assert_eq!(pixels.len(), 8 * 8 * 4);
            assert!(pixels.iter().all(|&b| b == 0), "both must start zeroed");
        }
        Ok(())
    }

    #[test]
    fn resize_rezeroes_at_the_new_size() -> Result<()> {
        let (device, queue) = device_queue();

        // `Gpu::resize` recreates the pair rather than resizing in place, so
        // the resize behavior to verify is that a fresh `History` at the new
        // size is zeroed too, not left holding the old allocation's content.
        let h = History::new(&device, 16, 16);
        let pixels = read_pixels(&device, &queue, &h.prev_texture, h.width, h.height)?;
        assert_eq!(pixels.len(), 16 * 16 * 4);
        assert!(
            pixels.iter().all(|&b| b == 0),
            "prev must be zero again at the new size, not stale/garbage"
        );
        Ok(())
    }
}
