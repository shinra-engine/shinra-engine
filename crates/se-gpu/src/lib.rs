//! Replaying a recorded frame on WebGPU.
//!
//! The payoff of letting the GPU decide the CPU layout is one line, in
//! [`Frame::draw`]: a component column is handed to `write_buffer` as the
//! slice it already is. No repacking, no per-field copy — the vertex attribute
//! offsets *are* the record's std430 offsets, because `se-contract` generated
//! both from one `contract.wit`.
//!
//! Nothing here knows a game. The vertex layout is built from the contract
//! table at run time, so one binary renders any game — which is the whole
//! reason the host does not compile a `contract.rs` in.

pub mod command;
mod pipeline;
pub mod replay;
pub mod resources;

pub use replay::Report;
pub use resources::Resource;
use resources::Loaded;

use anyhow::Result;
use std::collections::HashMap;
use se_vm::world::Meta;

pub struct Target {
    pub width: u32,
    pub height: u32,
    /// `copy_texture_to_buffer` wants rows aligned to 256 bytes; an image does
    /// not. The padding lives in the readback buffer and is dropped on the way
    /// out, so nothing above this ever sees it.
    pub row_bytes: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    target: Target,
    module: wgpu::ShaderModule,
    pipe_layout: wgpu::PipelineLayout,
    /// Keyed by (instance column, has-mesh); filled on first use.
    pipelines: HashMap<(u32, bool), wgpu::RenderPipeline>,
    bind_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    instances: wgpu::Buffer,
    resources: Vec<Option<Loaded>>,
    sampler: wgpu::Sampler,
    /// 1x1 white, bound when a draw names no texture.
    white: wgpu::TextureView,
}

/// Bytes the instance buffer holds, shared by every draw of a frame.
pub const INSTANCE_BYTES: u32 = 4096 * 64;
/// Uniform slots a frame can use; one per draw.
pub const UNIFORM_SLOTS: u32 = 64;
/// Bytes per uniform slot (the dynamic-offset alignment).
pub const UNIFORM_BYTES: u32 = 256;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Gpu {
    /// `shader` is `contract.wgsl` followed by the module's own `se.wgsl`, so
    /// the shader's instance input is the component record itself.
    pub fn new(
        meta: &Meta,
        shader: &str,
        width: u32,
        height: u32,
    ) -> Result<Gpu> {
        command::check_layout(meta)?;

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .map_err(|e| anyhow::anyhow!("no WebGPU adapter: {e:?}"))?;
        let adapter_name = adapter.get_info().name.clone();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("contract.wgsl + se.wgsl"),
            source: wgpu::ShaderSource::Wgsl(shader.into()),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("present"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let row_bytes = (width * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row_bytes * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniform"),
            size: (UNIFORM_BYTES * UNIFORM_SLOTS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("a column, verbatim"),
            size: INSTANCE_BYTES as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white = resources::upload_texture(
            &device,
            &queue,
            &resources::ImageData { width: 1, height: 1, rgba: vec![255; 4] },
        )
        .view;

        Ok(Gpu {
            device,
            queue,
            adapter_name,
            target: Target { width, height, row_bytes, texture, view, readback },
            module,
            pipe_layout: layout,
            pipelines: HashMap::new(),
            bind_layout,
            uniform,
            instances,
            resources: Vec::new(),
            sampler,
            white,
        })
    }

    /// The pipeline for a draw of `column`, built on first use.
    pub(crate) fn ensure_pipeline(&mut self, meta: &Meta, column: u32, with_mesh: bool) -> Result<()> {
        if !self.pipelines.contains_key(&(column, with_mesh)) {
            let p = pipeline::build(
                &self.device,
                &self.pipe_layout,
                &self.module,
                meta,
                column,
                with_mesh,
                FORMAT,
            )?;
            self.pipelines.insert((column, with_mesh), p);
        }
        Ok(())
    }

    /// Register (or replace) the mesh or texture an asset id names.
    pub fn set_resource(&mut self, id: u32, r: Resource) {
        let loaded = match r {
            Resource::Mesh(m) => Loaded::Mesh(resources::upload_mesh(&self.device, &m)),
            Resource::Texture(t) => {
                Loaded::Texture(resources::upload_texture(&self.device, &self.queue, &t))
            }
        };
        let id = id as usize;
        if self.resources.len() <= id {
            self.resources.resize_with(id + 1, || None);
        }
        self.resources[id] = Some(loaded);
    }

    pub fn adapter(&self) -> &str {
        &self.adapter_name
    }

    pub fn size(&self) -> (u32, u32) {
        (self.target.width, self.target.height)
    }

    /// Re-make the target at a new size. A terminal viewport changes shape
    /// when the window does and again when the glyph set does, and a frame
    /// rendered at the old size fills a corner of the panel.
    pub fn resize(&mut self, width: u32, height: u32) {
        if (width, height) == (self.target.width, self.target.height) || width == 0 || height == 0 {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("present"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let row_bytes = (width * 4).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row_bytes * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.target = Target { width, height, row_bytes, texture, view, readback };
    }
}
