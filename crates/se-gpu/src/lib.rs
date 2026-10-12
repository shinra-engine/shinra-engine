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
pub mod replay;

pub use replay::Report;

use anyhow::{bail, Context, Result};
use se_contract::layout::Ty;
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
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    instances: wgpu::Buffer,
    /// The column the pipeline's vertex layout was built for.
    instance_column: u32,
}

/// Instances the instance buffer holds.
pub const INSTANCE_CAP: u32 = 4096;
/// Bytes the uniform buffer holds.
pub const UNIFORM_BYTES: u32 = 256;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn vertex_format(ty: Ty) -> Result<wgpu::VertexFormat> {
    Ok(match ty {
        Ty::F32 => wgpu::VertexFormat::Float32,
        Ty::Vec(2) => wgpu::VertexFormat::Float32x2,
        Ty::Vec(3) => wgpu::VertexFormat::Float32x3,
        Ty::Vec(4) => wgpu::VertexFormat::Float32x4,
        Ty::I32 => wgpu::VertexFormat::Sint32,
        Ty::U32 => wgpu::VertexFormat::Uint32,
        Ty::Span => wgpu::VertexFormat::Uint32x2,
        other => bail!("no vertex format for {other:?}"),
    })
}

impl Gpu {
    /// `shader` is `contract.wgsl` followed by the module's own `se.wgsl`, so
    /// the shader's instance input is the component record itself.
    pub fn new(
        meta: &Meta,
        shader: &str,
        instance_column: u32,
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

        // Built from the contract, not from a generated constant: the host
        // must render a game whose contract it never compiled.
        let rec = meta
            .components
            .get(instance_column as usize)
            .with_context(|| format!("no component at column {instance_column}"))?;
        let attrs: Vec<wgpu::VertexAttribute> = rec
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                Ok(wgpu::VertexAttribute {
                    format: vertex_format(f.ty)?,
                    offset: f.offset as u64,
                    shader_location: i as u32,
                })
            })
            .collect::<Result<_>>()?;

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: rec.size as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
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
            size: UNIFORM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("a column, verbatim"),
            size: (rec.size as u64) * INSTANCE_CAP as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Gpu {
            device,
            queue,
            adapter_name,
            target: Target { width, height, row_bytes, texture, view, readback },
            pipeline,
            bind_layout,
            uniform,
            instances,
            instance_column,
        })
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
