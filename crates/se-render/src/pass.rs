//! Turning one `PassDef` into something the GPU can run.
//!
//! Split from `graph.rs` because the two halves answer different questions:
//! that file decides what order passes run in and hands them a frame, this one
//! decides what a pass *is* — its pipeline, its bindings, its instance buffer.
//! Nothing here knows about ordering, and nothing there knows about wgpu
//! pipeline layout.

use crate::attrs::{self, Instancing, Uniform};
use crate::graph::Scene;
use crate::image_asset::{self, ImageTexture};
use crate::mesh::{self, Mesh};
use crate::prelude_wgsl::{compose, FS_ENTRY, VS_FULLSCREEN, VS_MAIN};
use crate::targets::Targets;
use anyhow::{anyhow, bail, Context, Result};
use se_host::registry::PassDef;
use wgpu::{BindGroupLayout, Device, Queue, RenderPipeline};


#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Globals {
    pub(crate) resolution: [f32; 2],
    pub(crate) time: f32,
    pub(crate) dt: f32,
    pub(crate) frame: f32,
    pub(crate) _pad: [f32; 3],
}

pub(crate) struct Instanced {
    pub(crate) component: String,
    pub(crate) mesh: Mesh,
    pub(crate) layout: Instancing,
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) capacity: u64,
}

pub(crate) struct Compiled {
    pub(crate) pipeline: RenderPipeline,
    pub(crate) reads_layout: Option<BindGroupLayout>,
    /// Decoded once at compile time — an asset does not change until the
    /// module providing it is swapped, which rebuilds the graph anyway.
    pub(crate) textures: Vec<ImageTexture>,
    pub(crate) instanced: Option<Instanced>,
    pub(crate) uniform: Option<(String, Uniform, wgpu::Buffer, wgpu::BindGroup)>,
}


pub(crate) fn uniform_entry(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Copy the component column straight into the instance buffer. The column is
/// already dense and already strided the way the GPU wants, so this is a memcpy.
pub(crate) fn upload_instances(
    device: &Device,
    queue: &Queue,
    inst: &mut Instanced,
    scene: &Scene,
) -> Result<u32> {
    let Some(rec) = scene.space.records(&inst.component) else { return Ok(0) };
    let n = rec.len as u32;
    if n == 0 {
        return Ok(0);
    }
    let bytes = rec.bytes;
    let need = bytes.len() as u64;
    if need > inst.capacity {
        let cap = need.next_power_of_two().max(1024);
        inst.buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("se.instances"),
            size: cap,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        inst.capacity = cap;
    }
    queue.write_buffer(&inst.buffer, 0, bytes);
    Ok(n)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile(
    device: &Device,
    queue: &Queue,
    p: &PassDef,
    targets: &Targets,
    scene: &Scene,
    globals: &BindGroupLayout,
    uniform_layout: &BindGroupLayout,
) -> Result<Compiled> {
    if p.color.is_empty() {
        bail!("writes no colour buffer");
    }

    // Instancing, if this pass draws entities.
    let instanced = match p.draw.kind {
        se_abi::DrawKind::Fullscreen => None,
        se_abi::DrawKind::Instanced => {
            let component = p
                .draw
                .instance_of
                .clone()
                .ok_or_else(|| anyhow!("an instanced draw must name the component it draws"))?;
            let layout = scene
                .space
                .layout(&component)
                .ok_or_else(|| anyhow!("draws `{component}`, which the bundle does not define"))?;
            let name = p.draw.mesh.clone().unwrap_or_default();
            let m = Mesh::load(device, &name, scene.asset(&name))?;
            Some(Instanced {
                component,
                mesh: m,
                layout: attrs::instancing(layout)?,
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("se.instances"),
                    size: 1024,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                capacity: 1024,
            })
        }
    };

    // A component read as a uniform — a camera, most of the time.
    let uniform = match &p.uniform_of {
        None => None,
        Some(name) => {
            let l = scene
                .space
                .layout(name)
                .ok_or_else(|| anyhow!("reads `{name}` as a uniform, which the bundle does not define"))?;
            let u = attrs::uniform(l)?;
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("{}.uniform", p.name)),
                size: u.size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("{}.uniform", p.name)),
                layout: uniform_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
            });
            Some((name.clone(), u, buf, bind))
        }
    };

    let src = compose(
        p,
        instanced.as_ref().map(|i| &i.layout),
        uniform.as_ref().map(|(_, u, _, _)| u),
    );
    // A pass whose shader will not compile shows up as a black frame and a
    // validation error with no source attached, so make the source reachable.
    if let Some(dir) = std::env::var_os("SE_DUMP_WGSL") {
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("{}.wgsl", p.name)), &src);
    }
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&p.name),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });

    // Images this pass samples, decoded now.
    let mut textures = Vec::new();
    for name in &p.textures {
        textures.push(
            image_asset::load(device, queue, name, scene.asset(name))
                .with_context(|| format!("texture `{name}`"))?,
        );
    }

    let reads_layout = if p.reads.is_empty() && p.textures.is_empty() {
        None
    } else {
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        }];
        for (i, name) in p.reads.iter().enumerate() {
            let t = targets.get(name)?;
            if !t.def.sampled {
                bail!("reads `{name}`, which buffer.rs did not mark sampled");
            }
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: i as u32 + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        for i in 0..p.textures.len() {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: (p.reads.len() + i + 1) as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        Some(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{}.reads", p.name)),
            entries: &entries,
        }))
    };

    // Groups are positional, so an absent group still occupies its slot.
    let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("se.empty"),
        entries: &[],
    });
    let mut layouts: Vec<&BindGroupLayout> = vec![globals];
    if reads_layout.is_some() || uniform.is_some() {
        layouts.push(reads_layout.as_ref().unwrap_or(&empty));
    }
    if uniform.is_some() {
        layouts.push(uniform_layout);
    }
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&p.name),
        bind_group_layouts: &layouts,
        push_constant_ranges: &[],
    });

    let color_targets: Vec<Option<wgpu::ColorTargetState>> = p
        .color
        .iter()
        .map(|n| {
            targets.get(n).map(|t| {
                Some(wgpu::ColorTargetState {
                    format: t.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
        })
        .collect::<Result<_>>()?;

    let depth_stencil = match &p.depth {
        Some(n) => Some(wgpu::DepthStencilState {
            format: targets.get(n)?.format,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        None => None,
    };

    let instance_layout = instanced.as_ref().map(|i| wgpu::VertexBufferLayout {
        array_stride: i.layout.stride,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &i.layout.attributes,
    });
    let buffers: Vec<wgpu::VertexBufferLayout> = match &instance_layout {
        None => vec![],
        Some(il) => vec![mesh::vertex_layout(), il.clone()],
    };

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&p.name),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some(if instanced.is_some() { VS_MAIN } else { VS_FULLSCREEN }),
            compilation_options: Default::default(),
            buffers: &buffers,
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(FS_ENTRY),
            compilation_options: Default::default(),
            targets: &color_targets,
        }),
        multiview: None,
        cache: None,
    });

    Ok(Compiled { pipeline, reads_layout, textures, instanced, uniform })
}
