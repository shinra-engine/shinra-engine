//! Getting the presented buffer back to the CPU.
//!
//! The render side has no window, so "show" means handing pixels to whoever
//! asked — today a terminal. Copying every frame is not free, but at terminal
//! resolutions the frame is a few hundred kilobytes and the graph is the
//! expensive part.
//!
//! Encoding the copy and waiting for it are separate: a native host owns a
//! thread and can block on it, a browser owns a callback slot on an event loop
//! and blocking it is what stops the callback from ever arriving. Both routes
//! share [`encode`] and [`Pending::take`]; only the wait differs.

use crate::targets::Target;
use anyhow::{bail, Result};
use wgpu::{Device, Queue};

/// One frame of 8-bit RGBA, tightly packed.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

const ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// A copy that is in flight. Nothing has been waited on yet.
pub struct Pending {
    buf: wgpu::Buffer,
    w: u32,
    h: u32,
    padded: u32,
    unpadded: u32,
}

impl Pending {
    /// Strip the row padding into a tight frame. Only valid once mapped.
    pub fn take(self) -> Frame {
        let data = self.buf.slice(..).get_mapped_range();
        let mut rgba = Vec::with_capacity((self.unpadded * self.h) as usize);
        for row in 0..self.h {
            let off = (row * self.padded) as usize;
            rgba.extend_from_slice(&data[off..off + self.unpadded as usize]);
        }
        drop(data);
        self.buf.unmap();

        Frame { width: self.w, height: self.h, rgba }
    }
}

/// Record and submit the texture-to-buffer copy. Returns before it completes.
pub fn encode(device: &Device, queue: &Queue, target: &Target) -> Result<Pending> {
    let bpp = match target.format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => 4u32,
        other => bail!(
            "the presented buffer is {other:?}; present an Rgba8Unorm buffer, or add a pass that converts to one"
        ),
    };
    let (w, h) = target.size;
    let unpadded = w * bpp;
    let padded = unpadded.div_ceil(ALIGN) * ALIGN;

    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("se.readback"),
        size: (padded * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut enc =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("se.readback") });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target.write_texture(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);

    Ok(Pending { buf, w, h, padded, unpadded })
}

#[cfg(not(target_arch = "wasm32"))]
pub fn read(device: &Device, queue: &Queue, target: &Target) -> Result<Frame> {
    let p = encode(device, queue, target)?;

    let (tx, rx) = std::sync::mpsc::channel();
    p.buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    match rx.recv() {
        Ok(Ok(())) => Ok(p.take()),
        Ok(Err(e)) => bail!("mapping the readback buffer failed: {e}"),
        Err(_) => bail!("the readback buffer was never mapped"),
    }
}

/// The same read, waited on the event loop instead of on a thread.
///
/// `device.poll` does not drive the WebGPU backend — the browser calls the
/// `map_async` callback itself — so the only correct thing to do here is yield.
#[cfg(target_arch = "wasm32")]
pub async fn read_async(device: &Device, queue: &Queue, target: &Target) -> Result<Frame> {
    let p = encode(device, queue, target)?;

    let (tx, rx) = futures_channel::oneshot::channel();
    p.buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::Poll);
    match rx.await {
        Ok(Ok(())) => Ok(p.take()),
        Ok(Err(e)) => bail!("mapping the readback buffer failed: {e}"),
        Err(_) => bail!("the readback buffer was never mapped"),
    }
}
