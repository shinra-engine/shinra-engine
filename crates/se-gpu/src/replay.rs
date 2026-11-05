//! Walking the command list and issuing it.
//!
//! There is no graph interpreter here and no scheduling decision. The module
//! already decided; the host's whole job is to turn each command into the
//! WebGPU call it names, and to hand the GPU the column bytes exactly as the
//! stage left them.

use crate::command::{Command, BEGIN_PASS, DRAW_FULLSCREEN, DRAW_INSTANCED, END_PASS, NONE};
use crate::resources::Loaded;
use crate::{Gpu, GPU_BUDGET, INSTANCE_BYTES, UNIFORM_BYTES, UNIFORM_SLOTS};
use anyhow::{bail, Context, Result};
use se_vm::world::{Meta, World, COLUMN_BYTES};
use std::time::{Duration, Instant};

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
        let mut clear = wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let mut draws: Vec<&Command> = Vec::new();

        for c in cmds {
            match c.kind {
                BEGIN_PASS => {
                    report.passes += 1;
                    report.clear = c.clear;
                    clear = wgpu::Color {
                        r: c.clear[0] as f64,
                        g: c.clear[1] as f64,
                        b: c.clear[2] as f64,
                        a: c.clear[3] as f64,
                    };
                }
                DRAW_INSTANCED | DRAW_FULLSCREEN => draws.push(c),
                END_PASS => {}
                k => {
                    bail!("render.wasm recorded command kind {k}, which this host has no rule for")
                }
            }
        }
        if report.passes == 0 {
            bail!("the recorded frame opens no pass")
        }

        if draws.len() > UNIFORM_SLOTS as usize {
            bail!(
                "frame has {} draws but only {UNIFORM_SLOTS} uniform slots",
                draws.len()
            )
        }

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
                self.ensure_pipeline(meta, d.column, d.mesh != NONE)?;
            } else {
                self.ensure_pipeline(meta, NONE, false)?;
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

        // One bind group per draw per frame: its texture differs. Cheap at
        // this scale; cache by texture id if draws ever number in thousands.
        let binds: Vec<wgpu::BindGroup> = draws.iter().map(|d| self.bind_for(d)).collect();

        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            for (k, d) in draws.iter().enumerate() {
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
                        pass.set_pipeline(&self.pipelines[&(d.column, mesh.is_some())]);
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
                        pass.set_pipeline(&self.pipelines[&(NONE, false)]);
                        pass.set_bind_group(0, &binds[k], &[slot]);
                        pass.draw(0..3, 0..1);
                    }
                    _ => {}
                }
            }
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

    /// Uniform, the draw's texture (or white) and the sampler.
    fn bind_for(&self, d: &Command) -> wgpu::BindGroup {
        let view = match self.resources.get(d.texture as usize) {
            Some(Some(Loaded::Texture(t))) if d.texture != NONE => &t.view,
            _ => &self.white,
        };
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
        })
    }

    /// The rendered image, RGBA8. Offscreen for now — a window is the same
    /// replay against a surface texture.
    ///
    /// Adds its elapsed time to `last_cost` rather than replacing it, so
    /// after a `replay()` then `pixels()` call pair, `last_cost()` reflects
    /// both.
    pub fn pixels(&self) -> Result<Vec<u8>> {
        let start = Instant::now();
        let result = self.pixels_with_budget(GPU_BUDGET);
        self.last_cost.set(self.last_cost.get() + start.elapsed());
        result
    }

    /// Cost of the most recent `replay()` + `pixels()` call pair. Resets at
    /// the start of each `replay()` call, so this is never a running total
    /// across many frames.
    pub fn last_cost(&self) -> std::time::Duration {
        self.last_cost.get()
    }

    /// `pixels()`'s body, with the wall-clock budget as a parameter. A test
    /// can pass `Duration::ZERO` to force `wgpu::PollError::Timeout`
    /// deterministically — a submission just queued cannot be seen complete
    /// in a zero-length wait — without blocking for the real `GPU_BUDGET`.
    pub fn pixels_with_budget(&self, budget: Duration) -> Result<Vec<u8>> {
        let slice = self.target.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(budget),
            })
            .map_err(|e| {
                if matches!(e, wgpu::PollError::Timeout) {
                    anyhow::Error::new(e)
                        .context(format!("GPU replay exceeded its {budget:?} budget"))
                } else {
                    e.into()
                }
            })?;
        let padded = slice.get_mapped_range().to_vec();
        self.target.readback.unmap();
        // Drop the row padding the copy required.
        let w = (self.target.width * 4) as usize;
        let stride = self.target.row_bytes as usize;
        let mut out = Vec::with_capacity(w * self.target.height as usize);
        for y in 0..self.target.height as usize {
            out.extend_from_slice(&padded[y * stride..y * stride + w]);
        }
        Ok(out)
    }
}
