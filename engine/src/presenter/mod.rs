#![allow(dead_code)]

pub mod terminal;
pub mod window;

pub struct FrameCtx<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub texture: &'a wgpu::Texture, // engine's offscreen color target
    pub width: u32,
    pub height: u32,
}

pub trait Presenter {
    fn present(&mut self, ctx: &mut FrameCtx<'_>);
}
