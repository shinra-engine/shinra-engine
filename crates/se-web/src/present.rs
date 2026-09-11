//! Where the finished frame goes.
//!
//! The render side has no window on either route — it fills buffers and
//! something else decides what "show" means. Natively that is a terminal; here
//! it is a `<canvas>`, and there are two ways to reach one.
//!
//! The presented buffer is a bundle's own declaration, so whether it can *be*
//! the swapchain is the bundle's choice and not the engine's. When it can —
//! screen-sized, `Rgba8Unorm`, one layer — the texture is copied into the
//! swapchain image and nothing crosses to the CPU. When it cannot, the frame is
//! read back and written into a 2D context instead.
//!
//! The choice has to be made before the canvas is touched, because a canvas
//! hands out one kind of context and never another: asking for `webgpu`
//! forecloses `2d` for the lifetime of the element.

use anyhow::{anyhow, Result};
use se_host::registry::BufferDef;
use se_render::Gpu;
use wasm_bindgen::{Clamped, JsCast, JsValue};
use web_sys::HtmlCanvasElement;

/// How the presented buffer reaches the canvas.
pub enum Path {
    /// Copied into the swapchain image. No CPU copy.
    Surface,
    /// Read back and written into a 2D context.
    Blit,
}

/// Decide from the bundle's own declaration, before any GPU work.
///
/// `Rgba8UnormSrgb` is deliberately not on the swapchain list even though the
/// readback path accepts it: WebGPU's canvas formats are `bgra8unorm`,
/// `rgba8unorm` and `rgba16float`, and configuring a surface with anything
/// else is a validation error rather than a slower path.
pub fn choose(present: &BufferDef, screen: (u32, u32)) -> Path {
    let copyable = matches!(present.format, se_abi::Format::Rgba8Unorm)
        && present.count <= 1
        && present.resolve(screen) == screen;
    if copyable {
        Path::Surface
    } else {
        Path::Blit
    }
}

fn js(e: JsValue) -> anyhow::Error {
    anyhow!("{}", String::from(js_sys::JSON::stringify(&e).map(String::from).unwrap_or_default()))
}

/// A device able to fill this canvas, and the surface if there is to be one.
///
/// The path is decided here, before the canvas has a context, and the device
/// is acquired to match: an adapter that must reach a canvas is requested
/// compatible with that canvas's surface, and one that only reads back is
/// headless. Whoever owns the game keeps both — the device is not per frame.
pub async fn acquire(
    canvas: &HtmlCanvasElement,
    present: &BufferDef,
    screen: (u32, u32),
) -> Result<(Gpu, Option<wgpu::Surface<'static>>)> {
    match choose(present, screen) {
        Path::Surface => {
            let format = se_render::targets::wgpu_format(present.format);
            let (surface, gpu) = surface_on(canvas, format, screen).await?;
            Ok((gpu, Some(surface)))
        }
        // Headless is right here: nothing is being presented to, only read.
        Path::Blit => Ok((Gpu::new_async().await?, None)),
    }
}

/// A surface on the canvas, and a device able to present to it.
async fn surface_on(
    canvas: &HtmlCanvasElement,
    format: wgpu::TextureFormat,
    size: (u32, u32),
) -> Result<(wgpu::Surface<'static>, Gpu)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(|e| anyhow!("no WebGPU surface on this canvas: {e}"))?;
    // An adapter that has to reach a canvas must be requested compatible with
    // that canvas's surface — which is the whole of what ticket 31 added.
    let gpu = Gpu::new_for_surface(&instance, &surface).await?;
    surface.configure(
        &gpu.device,
        &wgpu::SurfaceConfiguration {
            // COPY_DST because the graph draws into the bundle's own target and
            // this image only receives it.
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0,
            height: size.1,
            present_mode: wgpu::PresentMode::Fifo,
            // `Auto` rather than a mode read off the adapter: `Gpu` does not
            // keep the adapter, and this is what the backend would pick anyway.
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        },
    );
    Ok((surface, gpu))
}

/// Copy the presented target into the swapchain image and show it.
pub fn to_swapchain(surface: &wgpu::Surface, gpu: &Gpu, target: &se_render::Target) -> Result<()> {
    let image = surface
        .get_current_texture()
        .map_err(|e| anyhow!("no swapchain image: {e}"))?;
    let (w, h) = target.size;
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("se.present") });
    enc.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: target.write_texture(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &image.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([enc.finish()]);
    image.present();
    Ok(())
}

/// Write a read-back frame into the canvas's 2D context.
pub fn to_2d(canvas: &HtmlCanvasElement, frame: &mut se_render::Frame) -> Result<()> {
    // The frame is whatever size the bundle asked for, which is the reason
    // this path exists at all; the canvas follows it rather than the reverse.
    canvas.set_width(frame.width);
    canvas.set_height(frame.height);
    let ctx = canvas
        .get_context("2d")
        .map_err(js)?
        .ok_or_else(|| anyhow!("this canvas gave out no 2d context"))?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .map_err(|_| anyhow!("that was not a 2d context"))?;
    let img = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
        Clamped(&mut frame.rgba),
        frame.width,
        frame.height,
    )
    .map_err(js)?;
    ctx.put_image_data(&img, 0.0, 0.0).map_err(js)
}
