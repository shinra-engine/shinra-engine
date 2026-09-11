//! Executing `render/*.so`'s nodes and edges.
//!
//! Pure: the graph reads component columns and asset bytes and writes buffers.
//! It cannot spawn, cannot write data, and knows no time beyond the globals it
//! is handed — not by discipline, but because `se_register_graph` hands over
//! nodes and edges and there is no other call it could make.

use crate::pass::{compile, uniform_entry, upload_instances, Compiled, Globals};
use crate::targets::Targets;
use anyhow::{bail, Context, Result};
use se_host::registry::{AssetDef, GraphDef, PassDef};
use se_host::Space;
use std::collections::HashMap;
use wgpu::{BindGroupLayout, Device, Queue};

/// What the graph is allowed to look at.
///
/// A space and some bytes — not a world. The render side never learns what an
/// entity is, which is what lets a different pipeline fill the same spaces and
/// be drawn by the same module.
pub struct Scene<'a> {
    pub space: &'a dyn Space,
    pub assets: &'a [&'a AssetDef],
}

impl Scene<'_> {
    pub(crate) fn asset(&self, name: &str) -> Option<&[u8]> {
        self.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.bytes.as_slice())
    }
}

pub struct Graph {
    def: GraphDef,
    order: Vec<usize>,
    compiled: Vec<Compiled>,
    globals_layout: BindGroupLayout,
    uniform_layout: BindGroupLayout,
    globals_buf: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    /// Bind groups are positional: a pass with a uniform but no reads still
    /// has a group 1 in its layout, and leaving it unbound is a validation
    /// error rather than a no-op.
    empty_bind: wgpu::BindGroup,
}

/// Kahn's algorithm over the declared edges. Passes nobody ordered keep their
/// declaration order, so a graph with no edges behaves the obvious way.
fn topo(def: &GraphDef) -> Result<Vec<usize>> {
    let index: HashMap<&str, usize> =
        def.passes.iter().enumerate().map(|(i, p)| (p.name.as_str(), i)).collect();
    let n = def.passes.len();
    let mut adj = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for (from, to) in &def.edges {
        let (&a, &b) = match (index.get(from.as_str()), index.get(to.as_str())) {
            (Some(a), Some(b)) => (a, b),
            _ => bail!("edge `{from}` -> `{to}` names a pass this graph does not declare"),
        };
        adj[a].push(b);
        indeg[b] += 1;
    }
    let mut ready: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    ready.sort_unstable();
    let mut out = Vec::with_capacity(n);
    while !ready.is_empty() {
        let i = ready.remove(0);
        out.push(i);
        for &j in &adj[i] {
            indeg[j] -= 1;
            if indeg[j] == 0 {
                ready.push(j);
                ready.sort_unstable();
            }
        }
    }
    if out.len() != n {
        bail!("render graph `{}` has a cycle in its edges", def.name);
    }
    Ok(out)
}

impl Graph {
    pub fn build(
        device: &Device,
        queue: &Queue,
        def: &GraphDef,
        targets: &Targets,
        scene: &Scene,
    ) -> Result<Graph> {
        if def.passes.is_empty() {
            bail!("render graph `{}` declares no passes", def.name);
        }
        targets.get(&def.present)?;
        let order = topo(def)?;

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("se.globals"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("se.uniform"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("se.globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("se.globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() }],
        });

        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("se.empty"),
            entries: &[],
        });
        let empty_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("se.empty"),
            layout: &empty_layout,
            entries: &[],
        });

        let mut compiled = Vec::with_capacity(def.passes.len());
        for p in &def.passes {
            compiled.push(
                compile(device, queue, p, targets, scene, &globals_layout, &uniform_layout)
                    .with_context(|| format!("pass `{}`", p.name))?,
            );
        }

        Ok(Graph {
            def: def.clone(),
            order,
            compiled,
            globals_layout,
            uniform_layout,
            globals_buf,
            globals_bind,
            empty_bind,
        })
    }

    pub fn def(&self) -> &GraphDef {
        &self.def
    }

    /// Passes in the order they will run — what the IDE lists.
    pub fn order(&self) -> impl Iterator<Item = &PassDef> {
        self.order.iter().map(|&i| &self.def.passes[i])
    }

    pub fn recompile(
        &mut self,
        device: &Device,
        queue: &Queue,
        targets: &Targets,
        scene: &Scene,
    ) -> Result<()> {
        self.compiled.clear();
        for p in &self.def.passes {
            self.compiled.push(compile(
                device,
                queue,
                p,
                targets,
                scene,
                &self.globals_layout,
                &self.uniform_layout,
            )?);
        }
        Ok(())
    }

    pub fn render(
        &mut self,
        device: &Device,
        queue: &Queue,
        targets: &Targets,
        scene: &Scene,
        time: f64,
        dt: f32,
        frame: u64,
        pixel_aspect: f32,
    ) -> Result<()> {
        let (w, h) = targets.screen();
        // Report the apparent size, not the buffer size: a shader asking for
        // aspect wants to know the shape of what the viewer sees.
        let apparent_h = h as f32 * pixel_aspect.max(0.0001);
        queue.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&Globals {
                resolution: [w as f32, apparent_h],
                time: time as f32,
                dt,
                frame: frame as f32,
                _pad: [0.0; 3],
            }),
        );

        // Instance and uniform data are refreshed before anything is encoded,
        // so a pass never reads a buffer another pass is still filling.
        let mut counts: Vec<u32> = vec![0; self.compiled.len()];
        for (i, c) in self.compiled.iter_mut().enumerate() {
            if let Some(inst) = &mut c.instanced {
                counts[i] = upload_instances(device, queue, inst, scene)?;
            }
            if let Some((name, u, buf, _)) = &c.uniform {
                if let Some(bytes) = scene.space.records(name).and_then(|r| r.row(0)) {
                    queue.write_buffer(buf, 0, &u.pack(bytes));
                }
            }
        }

        let mut enc = device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("se.frame") });

        for &i in &self.order {
            let p = &self.def.passes[i];
            let c = &self.compiled[i];

            let reads_bind = match &c.reads_layout {
                None => None,
                Some(layout) => {
                    let views: Vec<_> = p
                        .reads
                        .iter()
                        .map(|n| targets.get(n).map(|t| t.read_view()))
                        .collect::<Result<_>>()?;
                    let mut entries = vec![wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&targets.sampler),
                    }];
                    for (k, v) in views.iter().enumerate() {
                        entries.push(wgpu::BindGroupEntry {
                            binding: k as u32 + 1,
                            resource: wgpu::BindingResource::TextureView(v),
                        });
                    }
                    for (k, t) in c.textures.iter().enumerate() {
                        entries.push(wgpu::BindGroupEntry {
                            binding: (p.reads.len() + k + 1) as u32,
                            resource: wgpu::BindingResource::TextureView(&t.view),
                        });
                    }
                    Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(&p.name),
                        layout,
                        entries: &entries,
                    }))
                }
            };

            let color_views: Vec<_> = p
                .color
                .iter()
                .map(|n| targets.get(n).map(|t| t.write_view()))
                .collect::<Result<_>>()?;
            let ops = wgpu::Operations {
                load: if p.load {
                    wgpu::LoadOp::Load
                } else {
                    wgpu::LoadOp::Clear(wgpu::Color {
                        r: p.clear[0] as f64,
                        g: p.clear[1] as f64,
                        b: p.clear[2] as f64,
                        a: p.clear[3] as f64,
                    })
                },
                store: wgpu::StoreOp::Store,
            };
            let attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = color_views
                .iter()
                .map(|v| {
                    Some(wgpu::RenderPassColorAttachment {
                        view: v,
                        depth_slice: None,
                        resolve_target: None,
                        ops,
                    })
                })
                .collect();

            let depth = match &p.depth {
                Some(n) => Some(targets.get(n)?),
                None => None,
            };
            let depth_attachment = depth.map(|t| wgpu::RenderPassDepthStencilAttachment {
                view: t.write_view(),
                depth_ops: Some(wgpu::Operations {
                    load: if p.load { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(1.0) },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            });

            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&p.name),
                color_attachments: &attachments,
                depth_stencil_attachment: depth_attachment,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&c.pipeline);
            rp.set_bind_group(0, &self.globals_bind, &[]);
            match &reads_bind {
                Some(b) => rp.set_bind_group(1, b, &[]),
                // Reserved by the layout whenever a uniform follows it.
                None if c.uniform.is_some() => rp.set_bind_group(1, &self.empty_bind, &[]),
                None => {}
            }
            if let Some((_, _, _, bind)) = &c.uniform {
                rp.set_bind_group(2, bind, &[]);
            }
            match &c.instanced {
                None => rp.draw(0..3, 0..1),
                Some(inst) => {
                    let n = counts[i];
                    if n > 0 {
                        rp.set_vertex_buffer(0, inst.mesh.vertices.slice(..));
                        rp.set_vertex_buffer(1, inst.buffer.slice(..));
                        rp.set_index_buffer(
                            inst.mesh.indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        rp.draw_indexed(0..inst.mesh.index_count, 0, 0..n);
                    }
                }
            }
        }

        queue.submit([enc.finish()]);
        Ok(())
    }
}

