//! Device acquisition.
//!
//! Headless by design. The render side never owns a window — it fills buffers,
//! and something else decides what "show" means. That is what lets the same
//! graph present to a terminal, and it is why a software adapter is a
//! first-class outcome here rather than a fallback to apologise for.
//!
//! Acquisition itself is async on both routes and there is one copy of it. What
//! differs is who waits: a native host owns a thread and blocks on it, a browser
//! owns a callback slot on an event loop and blocking that slot is exactly what
//! stops the adapter request from ever resolving. So [`Gpu::new`] — the blocking
//! wrapper — is native only, and the browser awaits [`Gpu::new_async`] or
//! [`Gpu::new_for_surface`] instead.

use anyhow::{Context, Result};
use wgpu::{Adapter, Device, DeviceDescriptor, Instance, InstanceDescriptor, Queue, Surface};

pub struct Gpu {
    pub device: Device,
    pub queue: Queue,
    pub adapter_name: String,
    pub is_software: bool,
}

impl Gpu {
    /// Block until the device is ready.
    ///
    /// Native only: on wasm32 the future can only progress on the event loop
    /// this would be blocking, so it would never return.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Result<Gpu> {
        pollster::block_on(Self::new_async())
    }

    /// Headless acquisition. Valid on both routes — reading pixels back needs
    /// no canvas.
    pub async fn new_async() -> Result<Gpu> {
        let instance = Instance::new(&InstanceDescriptor::default());
        acquire(&instance, None).await
    }

    /// Acquire an adapter that is able to present to `surface`.
    ///
    /// A headless adapter can render and be read back, but one that has to
    /// reach a canvas must be requested compatible with that canvas's surface.
    pub async fn new_for_surface(instance: &Instance, surface: &Surface<'_>) -> Result<Gpu> {
        acquire(instance, Some(surface)).await
    }

    pub fn wait(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

/// The one implementation. Both constructors are the choice of `compatible`.
async fn acquire(instance: &Instance, compatible: Option<&Surface<'_>>) -> Result<Gpu> {
    let adapter: Adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: compatible,
        })
        .await
        .context("no wgpu adapter — install a Vulkan driver, or mesa-vulkan-drivers for a software one")?;

    let info = adapter.get_info();
    let is_software = info.device_type == wgpu::DeviceType::Cpu;

    let (device, queue) = adapter
        .request_device(&DeviceDescriptor {
            label: Some("shinra"),
            required_features: wgpu::Features::empty(),
            // Whatever the adapter has. A software rasteriser reports
            // modest limits and the graph is expected to fit them.
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .context("request_device")?;

    // Validation errors would otherwise surface as a silent black frame,
    // which is the worst possible failure mode for a hot-swapped shader.
    device.on_uncaptured_error(std::sync::Arc::new(|e| {
        report(&format!("[shinra:gpu] {e}"));
    }));

    Ok(Gpu { device, queue, adapter_name: info.name, is_software })
}

#[cfg(not(target_arch = "wasm32"))]
fn report(msg: &str) {
    eprintln!("{msg}");
}

/// The browser has no stderr, and this is the one message that must not be
/// lost — without it a validation error is just a black frame.
#[cfg(target_arch = "wasm32")]
fn report(msg: &str) {
    web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(msg));
}
