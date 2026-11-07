//! Walking the command list and issuing it.
//!
//! There is no graph interpreter here and no scheduling decision. The module
//! already decided; the host's whole job is to turn each command into the
//! WebGPU call it names, and to hand the GPU the column bytes exactly as the
//! stage left them.

use crate::command::{Command, DRAW_FULLSCREEN, DRAW_INSTANCED, FLAG_PREV, NONE};
use crate::pass::{self, group_passes};
use crate::resources::Loaded;
use crate::{Gpu, INSTANCE_BYTES, UNIFORM_BYTES, UNIFORM_SLOTS};
use anyhow::{bail, Context, Result};
use se_vm::world::{Meta, World, COLUMN_BYTES};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// What one replayed frame did, so a caller can assert on it without reading
/// pixels back.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub passes: usize,
    pub draws: usize,
    pub instances: u32,
    pub clear: [f32; 4],
    /// Pipelines this `Gpu` has built so far.
    pub pipelines: usize,
}

fn u32_at(m: &[u8], at: u32) -> u32 {
    let a = at as usize;
    u32::from_le_bytes([m[a], m[a + 1], m[a + 2], m[a + 3]])
}

impl Gpu {
    /// Replay one recorded frame. `mem` is the shared linear memory — the
    /// world as it stands, not a copy of it.
    ///
    /// Times the call and stores it in `last_cost`, replacing any prior
    /// value — this is the cost of the most recent replay, not a running
    /// total.
    pub fn replay(
        &mut self,
        meta: &Meta,
        world: &World,
        mem: &[u8],
        cmds: &[Command],
    ) -> Result<Report> {
        let start = Instant::now();
        let result = self.replay_inner(meta, world, mem, cmds);
        self.last_cost.set(start.elapsed());
        result
    }

    fn replay_inner(
        &mut self,
        meta: &Meta,
        world: &World,
        mem: &[u8],
        cmds: &[Command],
    ) -> Result<Report> {
        let mut report = Report::default();
        let (groups, draws) = group_passes(cmds)?;
        if groups.is_empty() {
            bail!("the recorded frame opens no pass")
        }
        report.passes = groups.len();
        report.clear = groups.last().expect("non-empty").clear;

        if draws.len() > UNIFORM_SLOTS as usize {
            bail!(
                "frame has {} draws but only {UNIFORM_SLOTS} uniform slots",
                draws.len()
            )
        }

        // Resolve every pass's target view before any further borrow of
        // `self`: `target_view` takes `&mut self` (it may lazily create an
        // offscreen texture), and a render pass below borrows `self`
        // immutably (pipelines, instance buffer) for as long as it is open.
        let views: Vec<wgpu::TextureView> = groups
            .iter()
            .map(|g| self.target_view(g.target).clone())
            .collect();
        let (depths, draw_depth) = pass::resolve_depth(self, &groups, draws.len());

        // Uploads first: a render pass borrows the encoder, and a column is
        // read straight out of the arena. Each draw gets its own bytes.
        let mut next = 0u64;
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        for (k, d) in draws.iter().enumerate() {
            let mut range = (0u64, 0u64);
            if d.kind == DRAW_INSTANCED {
                let pool = world
                    .pool_of
                    .get(d.column as usize)
                    .copied()
                    .unwrap_or(u32::MAX);
                if let Some(cap) = world.pool_cap.get(pool as usize).copied() {
                    if d.count > cap {
                        let name = meta
                            .components
                            .get(d.column as usize)
                            .map_or("?", |c| c.name.as_str());
                        bail!(
                            "draw names {} rows of `{name}` but the pool holds only {cap}",
                            d.count
                        )
                    }
                }
                let desc = meta.columns_at + d.column * COLUMN_BYTES;
                let base = u32_at(mem, desc) as usize;
                let stride = u32_at(mem, desc + 8) as usize;
                let len = stride * d.count as usize;
                let off = next;
                let end = off + len as u64;
                if end > INSTANCE_BYTES as u64 {
                    bail!(
                        "the frame's instances need {end} bytes but the instance buffer holds {INSTANCE_BYTES}"
                    )
                }
                let bytes = mem
                    .get(base..base + len)
                    .context("the column a draw names runs past the memory")?;
                // The entire conversion from world to GPU. std430 offsets in,
                // std430 offsets out.
                if len > 0 {
                    self.queue.write_buffer(&self.instances, off, bytes);
                }
                next = end.div_ceil(4) * 4;
                range = (off, end);
                report.draws += 1;
                report.instances += d.count;
                self.ensure_pipeline(meta, d.column, d.mesh != NONE, draw_depth[k])?;
            } else {
                self.ensure_pipeline(meta, NONE, false, draw_depth[k])?;
            }
            ranges.push(range);
            self.check_assets(d)?;
            if d.uniform_len > UNIFORM_BYTES {
                bail!(
                    "uniform of {} bytes exceeds the {UNIFORM_BYTES}-byte uniform slot",
                    d.uniform_len
                )
            }
            if d.uniform_len > 0 {
                let u = d.uniform_off as usize;
                let bytes = mem
                    .get(u..u + d.uniform_len as usize)
                    .context("the uniform a draw names runs past the memory")?;
                self.queue
                    .write_buffer(&self.uniform, (k as u32 * UNIFORM_BYTES) as u64, bytes);
            }
        }
        report.pipelines = self.pipelines.len();

        // One bind group per draw per frame: its texture differs. A draw's
        // `sample` may only point to a target an earlier pass group of this
        // same frame already wrote, so bind groups are resolved pass by
        // pass, in order, tracking which target each finished group wrote.
        let mut written: HashMap<u32, usize> = HashMap::new();
        let mut binds: Vec<wgpu::BindGroup> = Vec::with_capacity(draws.len());
        for (gi, group) in groups.iter().enumerate() {
            for k in group.draws.clone() {
                binds.push(self.bind_for(draws[k], &written, &views)?);
            }
            written.insert(group.target, gi);
        }

        let mut enc = self.device.create_command_encoder(&Default::default());
        for (gi, group) in groups.iter().enumerate() {
            let clear = wgpu::Color {
                r: group.clear[0] as f64,
                g: group.clear[1] as f64,
                b: group.clear[2] as f64,
                a: group.clear[3] as f64,
            };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &views[gi],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: depths[gi].as_ref().map(|view| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            for k in group.draws.clone() {
                let d = draws[k];
                let slot = k as u32 * UNIFORM_BYTES;
                match d.kind {
                    DRAW_INSTANCED => {
                        // WebGPU rejects an empty buffer slice. A zero-row
                        // pool is nevertheless a normal state (for example,
                        // while a game is entering Play), and a zero-instance
                        // draw has no visible work to submit.
                        if d.count == 0 {
                            continue;
                        }
                        let mesh = match self.resources.get(d.mesh as usize) {
                            Some(Some(Loaded::Mesh(m))) => Some(m),
                            _ => None,
                        };
                        pass.set_pipeline(&self.pipelines[&(d.column, mesh.is_some(), group.depth)]);
                        pass.set_bind_group(0, &binds[k], &[slot]);
                        let (a, b) = ranges[k];
                        pass.set_vertex_buffer(0, self.instances.slice(a..b));
                        if let Some(m) = mesh {
                            // The mesh's own vertex count; `command.vertices`
                            // is ignored for a mesh draw.
                            pass.set_vertex_buffer(1, m.buffer.slice(..));
                            pass.draw(0..m.vertices, 0..d.count);
                        } else {
                            // Three is the fullscreen-triangle default, for a
                            // module that names no geometry of its own.
                            pass.draw(0..d.vertices.max(3), 0..d.count);
                        }
                    }
                    // Three vertices and no buffer: the shader builds the
                    // triangle. Clears, blits and post are this.
                    DRAW_FULLSCREEN => {
                        pass.set_pipeline(&self.pipelines[&(NONE, false, group.depth)]);
                        pass.set_bind_group(0, &binds[k], &[slot]);
                        pass.draw(0..3, 0..1);
                    }
                    _ => {}
                }
            }
        }

        // Every offscreen target this frame drew into gets its `cur` copied
        // into `prev`, in this same encoder, before submit — so the next
        // frame's FLAG_PREV read sees this frame's finished content, not a
        // stale or half-drawn one. The present target (0) keeps no history.
        let written_targets: HashSet<u32> =
            groups.iter().map(|g| g.target).filter(|&t| t != 0).collect();
        for id in written_targets {
            self.copy_history(&mut enc, id);
        }

        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.target.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.target.row_bytes),
                    rows_per_image: Some(self.target.height),
                },
            },
            wgpu::Extent3d {
                width: self.target.width,
                height: self.target.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        Ok(report)
    }

    /// A draw's `mesh` and `texture` must name resources of that kind.
    fn check_assets(&self, d: &Command) -> Result<()> {
        let kind = |id: u32| match self.resources.get(id as usize) {
            Some(Some(Loaded::Mesh(_))) => "mesh",
            Some(Some(Loaded::Texture(_))) => "texture",
            _ => "absent",
        };
        if d.mesh != NONE && kind(d.mesh) != "mesh" {
            bail!(
                "draw names asset {} as a mesh but it is {}",
                d.mesh,
                kind(d.mesh)
            )
        }
        if d.texture != NONE && kind(d.texture) != "texture" {
            bail!(
                "draw names asset {} as a texture but it is {}",
                d.texture,
                kind(d.texture)
            )
        }
        Ok(())
    }

    /// Uniform, the draw's texture (or an earlier pass's target, or that
    /// target's previous-frame history, or white) and the sampler. `written`
    /// maps a target id to the index into `views` of the most recent pass
    /// group, strictly before the one `d` belongs to, that wrote it this
    /// frame — a `DRAW_FULLSCREEN` naming an id missing from it is refused
    /// rather than reading the current or a future pass's target. A draw
    /// with `FLAG_PREV` set instead reads `sample`'s PREV texture directly,
    /// regardless of whether (or when) this frame writes that target's CUR.
    fn bind_for(
        &mut self,
        d: &Command,
        written: &HashMap<u32, usize>,
        views: &[wgpu::TextureView],
    ) -> Result<wgpu::BindGroup> {
        let view = if d.kind == DRAW_FULLSCREEN && d.render_flags & FLAG_PREV != 0 {
            if d.texture != NONE {
                bail!("a draw cannot set both `texture` and `sample`")
            }
            if d.sample == NONE || d.sample == 0 {
                bail!(
                    "draw sets FLAG_PREV but `sample` names no offscreen target to read history from"
                )
            }
            // Ensure the entry exists (a mutable call, dropped immediately)
            // before borrowing it immutably alongside the bind group's other
            // fields below.
            self.ensure_offscreen(d.sample);
            &self.offscreen[&d.sample].prev_view
        } else if d.kind == DRAW_FULLSCREEN && d.sample != NONE && d.sample != 0 {
            if d.texture != NONE {
                bail!("a draw cannot set both `texture` and `sample`")
            }
            let gi = written.get(&d.sample).ok_or_else(|| {
                anyhow::anyhow!(
                    "draw samples target {}, but no earlier pass this frame wrote it",
                    d.sample
                )
            })?;
            &views[*gi]
        } else {
            match self.resources.get(d.texture as usize) {
                Some(Some(Loaded::Texture(t))) if d.texture != NONE => &t.view,
                _ => &self.white,
            }
        };
        Ok(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniform,
                        offset: 0,
                        size: wgpu::BufferSize::new(UNIFORM_BYTES as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }))
    }
}
