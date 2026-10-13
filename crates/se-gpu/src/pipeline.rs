//! One render pipeline per (instance column, has-mesh). The vertex layout is
//! built from the contract table, so the host renders a game whose contract
//! it never compiled.

use crate::command::NONE;
use anyhow::{bail, Context, Result};
use se_contract::layout::Ty;
use se_vm::world::Meta;

/// `MeshVertex`: position then normal, both vec3.
pub const MESH_STRIDE: u64 = 24;

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

/// `column == NONE` builds the fullscreen variant: no vertex buffers at all.
pub(crate) fn build(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    meta: &Meta,
    column: u32,
    with_mesh: bool,
    format: wgpu::TextureFormat,
) -> Result<wgpu::RenderPipeline> {
    let mesh_attrs = [
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 14 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 15 },
    ];
    let mut attrs: Vec<wgpu::VertexAttribute> = Vec::new();
    let mut stride = 0u64;
    if column != NONE {
        let rec = meta
            .components
            .get(column as usize)
            .with_context(|| format!("no component at column {column}"))?;
        stride = rec.size as u64;
        for (i, f) in rec.fields.iter().enumerate() {
            attrs.push(wgpu::VertexAttribute {
                format: vertex_format(f.ty)?,
                offset: f.offset as u64,
                shader_location: i as u32,
            });
        }
    }
    let mut buffers = Vec::new();
    if column != NONE {
        buffers.push(wgpu::VertexBufferLayout {
            array_stride: stride,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &attrs,
        });
        if with_mesh {
            buffers.push(wgpu::VertexBufferLayout {
                array_stride: MESH_STRIDE,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &mesh_attrs,
            });
        }
    }
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    }))
}
