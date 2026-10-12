//! Walking the command list and issuing it.
//!
//! There is no graph interpreter here and no scheduling decision. The module
//! already decided; the host's whole job is to turn each command into the
//! WebGPU call it names, and to hand the GPU the column bytes exactly as the
//! stage left them.

use crate::command::{Command, BEGIN_PASS, DRAW_FULLSCREEN, DRAW_INSTANCED, END_PASS};
use crate::{Gpu, INSTANCE_CAP, UNIFORM_BYTES};
use anyhow::{bail, Context, Result};
use se_vm::world::{Meta, World, COLUMN_BYTES};

/// What one replayed frame did, so a caller can assert on it without reading
/// pixels back.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub passes: usize,
    pub draws: usize,
    pub instances: u32,
    pub clear: [f32; 4],
}

fn u32_at(m: &[u8], at: u32) -> u32 {
    let a = at as usize;
    u32::from_le_bytes([m[a], m[a + 1], m[a + 2], m[a + 3]])
}

impl Gpu {
    /// Replay one recorded frame. `mem` is the shared linear memory — the
    /// world as it stands, not a copy of it.
    pub fn replay(
        &self,
        meta: &Meta,
        world: &World,
        mem: &[u8],
        cmds: &[Command],
    ) -> Result<Report> {
        let mut report = Report::default();
        let mut clear = wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
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
                k => bail!("render.wasm recorded command kind {k}, which this host has no rule for"),
            }
        }
        if report.passes == 0 {
            bail!("the recorded frame opens no pass")
        }

        // Uploads first: a render pass borrows the encoder, and a column is
        // read straight out of the arena.
        for d in &draws {
            if d.kind == DRAW_INSTANCED {
                if d.column != self.instance_column {
                    bail!(
                        "the pipeline was built for column {} but the frame draws column {}",
                        self.instance_column,
                        d.column
                    )
                }
                if d.count > INSTANCE_CAP {
                    bail!(
                        "draw of {} instances exceeds the {INSTANCE_CAP} the instance buffer holds",
                        d.count
                    )
                }
                let pool = world.pool_of.get(d.column as usize).copied().unwrap_or(u32::MAX);
                if let Some(cap) = world.pool_cap.get(pool as usize).copied() {
                    if d.count > cap {
                        let name = meta
                            .components
                            .get(d.column as usize)
                            .map_or("?", |c| c.name.as_str());
                        bail!("draw names {} rows of `{name}` but the pool holds only {cap}", d.count)
                    }
                }
                let desc = meta.columns_at + d.column * COLUMN_BYTES;
                let base = u32_at(mem, desc) as usize;
                let stride = u32_at(mem, desc + 8) as usize;
                let len = stride * d.count as usize;
                let bytes = mem
                    .get(base..base + len)
                    .context("the column a draw names runs past the memory")?;
                // The entire conversion from world to GPU. std430 offsets in,
                // std430 offsets out.
                self.queue.write_buffer(&self.instances, 0, bytes);
                report.draws += 1;
                report.instances += d.count;
            }
            if d.uniform_len > UNIFORM_BYTES {
                bail!(
                    "uniform of {} bytes exceeds the {UNIFORM_BYTES}-byte uniform buffer",
                    d.uniform_len
                )
            }
            if d.uniform_len > 0 {
                let u = d.uniform_off as usize;
                let bytes = mem
                    .get(u..u + d.uniform_len as usize)
                    .context("the uniform a draw names runs past the memory")?;
                self.queue.write_buffer(&self.uniform, 0, bytes);
            }
        }

        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniform.as_entire_binding(),
            }],
        });

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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            for d in &draws {
                match d.kind {
                    DRAW_INSTANCED => {
                        pass.set_vertex_buffer(0, self.instances.slice(..));
                        // Three is the fullscreen-triangle default, for a
                        // module that names no geometry of its own.
                        pass.draw(0..d.vertices.max(3), 0..d.count);
                    }
                    // Three vertices and no buffer: the shader builds the
                    // triangle. Clears, blits and post are this.
                    DRAW_FULLSCREEN => pass.draw(0..3, 0..1),
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

    /// The rendered image, RGBA8. Offscreen for now — a window is the same
    /// replay against a surface texture.
    pub fn pixels(&self) -> Result<Vec<u8>> {
        let slice = self.target.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })?;
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
