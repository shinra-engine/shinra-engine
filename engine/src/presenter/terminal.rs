use crate::presenter::{FrameCtx, Presenter};
use crossterm::{cursor::MoveTo, execute};
use image::{DynamicImage, RgbaImage};
use std::io::stdout;
use viuer::Config;

pub struct TerminalPresenter {
    readback: wgpu::Buffer,
    pad_bytes_per_row: u32,
    width: u32,
    height: u32,
}

impl TerminalPresenter {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let unpadded = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let pad_bytes_per_row = unpadded.div_ceil(align) * align;
        let buffer_size = (pad_bytes_per_row * height) as u64;

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminal_readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            readback,
            pad_bytes_per_row,
            width,
            height,
        }
    }
}

impl Presenter for TerminalPresenter {
    fn present(&mut self, ctx: &mut FrameCtx<'_>) {
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("terminal_readback"),
            });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: ctx.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.pad_bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: ctx.width,
                height: ctx.height,
                depth_or_array_layers: 1,
            },
        );

        ctx.queue.submit([encoder.finish()]);

        let buffer_slice = self.readback.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());

        let data = buffer_slice.get_mapped_range();

        let width = self.width as usize;
        let height = self.height as usize;
        let pad_bpr = self.pad_bytes_per_row as usize;
        let real_bpr = width * 4;

        let mut pixels = Vec::with_capacity(width * height * 4);
        for row in 0..height {
            let start = row * pad_bpr;
            pixels.extend_from_slice(&data[start..start + real_bpr]);
        }

        drop(data);
        self.readback.unmap();

        let img = RgbaImage::from_raw(self.width, self.height, pixels).unwrap();

        let _ = execute!(stdout(), MoveTo(0, 0));

        let config = Config {
            absolute_offset: false,
            x: 0,
            y: 0,
            ..Default::default()
        };
        let _ = viuer::print(&DynamicImage::ImageRgba8(img), &config);
    }
}
