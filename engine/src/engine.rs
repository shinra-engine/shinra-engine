use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

pub struct Engine {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub color: wgpu::Texture,
    pub depth: wgpu::Texture,
    pub size: (u32, u32),
    pipeline: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    #[allow(dead_code)]
    camera_bgl: wgpu::BindGroupLayout,
    camera_bg: wgpu::BindGroup,
    object_bgl: wgpu::BindGroupLayout,
    object_slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    // Stores Arc<Mesh> alongside buffers so the mesh is kept alive and its
    // pointer is stable (no reuse by a different allocation).
    mesh_cache:
        HashMap<*const crate::mesh::Mesh, (Arc<crate::mesh::Mesh>, wgpu::Buffer, wgpu::Buffer)>,
}

impl Engine {
    /// Build a headless engine (no window/surface) at the given render size.
    pub fn new(width: u32, height: u32) -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("failed to find a suitable wgpu adapter");

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("failed to create wgpu device");

        Self::from_existing(device, queue, width, height)
    }

    /// Build an engine from an already-created device and queue.
    /// Use this when the caller needs to share the device with a surface presenter.
    pub fn from_existing(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Self {
        let color = Self::make_color(&device, width, height);
        let depth = Self::make_depth(&device, width, height);

        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let object_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let identity: [f32; 16] = glam::Mat4::IDENTITY.to_cols_array();
        let camera_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&identity),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera_bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[&camera_bgl, &object_bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[crate::mesh::Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            device,
            queue,
            color,
            depth,
            size: (width, height),
            pipeline,
            camera_buf,
            camera_bgl,
            camera_bg,
            object_bgl,
            object_slots: Vec::new(),
            mesh_cache: HashMap::new(),
        }
    }

    pub fn render(&mut self, scene: &crate::scene::Scene) {
        use crate::scene::{MeshHandle, Model};

        let vp: [f32; 16] = scene.camera.view_proj().to_cols_array();
        self.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&vp));

        // Collect drawables in spawn (query) order — stable within a frame.
        let drawables: Vec<(Arc<crate::mesh::Mesh>, glam::Mat4)> = scene
            .world
            .query::<(&MeshHandle, &Model)>()
            .iter()
            .map(|(_, (mh, m))| (Arc::clone(&mh.0), m.0))
            .collect();

        for (mesh, _) in &drawables {
            let mesh_ptr = Arc::as_ptr(mesh);
            if !self.mesh_cache.contains_key(&mesh_ptr) {
                let vbuf = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("vbuf"),
                        contents: bytemuck::cast_slice(&mesh.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                let ibuf = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("ibuf"),
                        contents: bytemuck::cast_slice(&mesh.indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });
                self.mesh_cache
                    .insert(mesh_ptr, (Arc::clone(mesh), vbuf, ibuf));
            }
        }

        // Grow object_slots to cover all drawables, then upload model matrices.
        let needed = drawables.len();
        while self.object_slots.len() < needed {
            let identity: [f32; 16] = glam::Mat4::IDENTITY.to_cols_array();
            let buf = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("object_model"),
                    contents: bytemuck::bytes_of(&identity),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("object_bg"),
                layout: &self.object_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            });
            self.object_slots.push((buf, bg));
        }
        for (i, (_, model)) in drawables.iter().enumerate() {
            let model_arr: [f32; 16] = model.to_cols_array();
            self.queue
                .write_buffer(&self.object_slots[i].0, 0, bytemuck::bytes_of(&model_arr));
        }

        let color_view = self
            .color
            .create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = self
            .depth
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.07,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bg, &[]);

            for (i, (mesh, _)) in drawables.iter().enumerate() {
                let mesh_ptr = Arc::as_ptr(mesh);
                let (_, vbuf, ibuf) = self.mesh_cache.get(&mesh_ptr).unwrap();
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_bind_group(1, &self.object_slots[i].1, &[]);
                pass.draw_indexed(0..mesh.indices.len() as u32, 0, 0..1);
            }
        }

        self.queue.submit([encoder.finish()]);
    }

    /// Reallocate color + depth textures at a new size.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.color = Self::make_color(&self.device, width, height);
        self.depth = Self::make_depth(&self.device, width, height);
        self.size = (width, height);
    }

    fn make_color(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn make_depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_init() {
        let engine = Engine::new(64, 64);
        assert_eq!(engine.size, (64, 64));

        let color_size = engine.color.size();
        assert_eq!(color_size.width, 64);
        assert_eq!(color_size.height, 64);
        assert_eq!(engine.color.format(), wgpu::TextureFormat::Rgba8UnormSrgb);

        let depth_size = engine.depth.size();
        assert_eq!(depth_size.width, 64);
        assert_eq!(depth_size.height, 64);
        assert_eq!(engine.depth.format(), wgpu::TextureFormat::Depth32Float);
    }

    #[test]
    fn engine_resize() {
        let mut engine = Engine::new(64, 64);
        engine.resize(128, 96);
        assert_eq!(engine.size, (128, 96));

        let color_size = engine.color.size();
        assert_eq!(color_size.width, 128);
        assert_eq!(color_size.height, 96);

        let depth_size = engine.depth.size();
        assert_eq!(depth_size.width, 128);
        assert_eq!(depth_size.height, 96);
    }

    #[test]
    fn engine_pipeline() {
        let engine = Engine::new(64, 64);
        assert_eq!(engine.camera_buf.size(), 64);
    }
}
