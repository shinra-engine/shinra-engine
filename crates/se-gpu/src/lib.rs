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
mod history;
mod pass;
mod pipeline;
mod readback;
pub mod replay;
pub mod resources;

pub use replay::Report;
pub use resources::Resource;
use resources::Loaded;

use anyhow::Result;
use std::cell::Cell;
use std::collections::HashMap;
use std::time::Duration;
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
    /// Offscreen targets 1..=8, created the first time a command names them
    /// and dropped wholesale by `resize()` so a stale size is never read.
    offscreen: HashMap<u32, history::History>,
    /// One `Depth32Float` view per target id (0 = present, 1..=8 offscreen),
    /// created the first time a depth-flagged pass names that target and
    /// dropped wholesale by `resize()` alongside `offscreen`, so a stale size
    /// is never read.
    depth: HashMap<u32, wgpu::TextureView>,
    module: wgpu::ShaderModule,
    pipe_layout: wgpu::PipelineLayout,
    /// Keyed by (instance column, has-mesh, depth-attached); filled on first
    /// use. Depth varies the key because the same column may be drawn both
    /// inside and outside a depth-flagged pass, and those need different
    /// pipeline state.
    pipelines: HashMap<(u32, bool, bool), wgpu::RenderPipeline>,
    bind_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    instances: wgpu::Buffer,
    resources: Vec<Option<Loaded>>,
    sampler: wgpu::Sampler,
    /// 1x1 white, bound when a draw names no texture.
    white: wgpu::TextureView,
    /// Cost of the most recent `replay()` + `pixels()` pair. `pixels()` takes
    /// `&self`, so this needs interior mutability to be updated from there.
    last_cost: Cell<Duration>,
}

/// Bytes the instance buffer holds, shared by every draw of a frame.
pub const INSTANCE_BYTES: u32 = 4096 * 64;
/// Uniform slots a frame can use; one per draw.
pub const UNIFORM_SLOTS: u32 = 64;
/// Bytes per uniform slot (the dynamic-offset alignment).
pub const UNIFORM_BYTES: u32 = 256;
/// Wall-clock bound on waiting for a replayed frame's GPU work to finish.
/// Two orders of magnitude above `run.rs`'s ~33ms `TARGET` render budget, so
/// normal variance never trips it while a genuinely hung driver is caught
/// well inside human-noticeable delay. A placeholder like `run.rs`'s own
/// `TARGET`/`WATCH`, not empirically measured the way `DEFAULT_FUEL` was.
pub const GPU_BUDGET: Duration = Duration::from_secs(2);

pub(crate) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

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
        let (texture, view) = Self::make_texture(
            &device,
            width,
            height,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
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
            offscreen: HashMap::new(),
            depth: HashMap::new(),
            module,
            pipe_layout: layout,
            pipelines: HashMap::new(),
            bind_layout,
            uniform,
            instances,
            resources: Vec::new(),
            sampler,
            white,
            last_cost: Cell::new(Duration::ZERO),
        })
    }

    /// The pipeline for a draw of `column` in a pass with the given depth
    /// flag, built on first use.
    pub(crate) fn ensure_pipeline(
        &mut self,
        meta: &Meta,
        column: u32,
        with_mesh: bool,
        depth: bool,
    ) -> Result<()> {
        if !self.pipelines.contains_key(&(column, with_mesh, depth)) {
            let p = pipeline::build(
                &self.device,
                &self.pipe_layout,
                &self.module,
                meta,
                column,
                with_mesh,
                FORMAT,
                depth,
            )?;
            self.pipelines.insert((column, with_mesh, depth), p);
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
        let (texture, view) = Self::make_texture(
            &self.device,
            width,
            height,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let row_bytes = (width * 4).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row_bytes * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.target = Target { width, height, row_bytes, texture, view, readback };
        // A cached offscreen target's size follows the present target's size
        // at the time it was created; a stale one must never be read back.
        self.offscreen.clear();
        self.depth.clear();
    }

    fn make_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        usage: wgpu::TextureUsages,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }

    /// Resolve a target id to its view: 0 is the present target, 1..=8 are
    /// offscreen, created lazily at the present target's current size the
    /// first time a command names them. `resize()` drops every offscreen
    /// target, so a stale size is never read.
    pub(crate) fn target_view(&mut self, id: u32) -> &wgpu::TextureView {
        if id == 0 {
            return &self.target.view;
        }
        self.ensure_offscreen(id);
        &self.offscreen[&id].cur_view
    }

    /// Lazily creates `id`'s history pair at the present target's current
    /// size, exactly once, so `target_view` and a `FLAG_PREV` read of
    /// `offscreen[&id].prev_view` agree on it. `pub(crate)` and separate from
    /// a `prev_view`-style accessor: a caller that also needs to borrow other
    /// `self` fields alongside the result (as `replay::bind_for` does for the
    /// bind group's uniform/sampler) calls this first, mutably, then borrows
    /// `offscreen[&id]` itself immutably — avoiding a borrow that would
    /// otherwise span both.
    pub(crate) fn ensure_offscreen(&mut self, id: u32) {
        if !self.offscreen.contains_key(&id) {
            let (width, height) = (self.target.width, self.target.height);
            let h = history::History::new(&self.device, width, height);
            self.offscreen.insert(id, h);
        }
    }

    /// Copies `id`'s `cur` texture into its `prev` texture, in `enc`. Call
    /// once per offscreen target a frame wrote, after every pass is encoded
    /// and before the encoder is submitted, so the next frame's `prev` read
    /// sees this frame's finished `cur`.
    pub(crate) fn copy_history(&self, enc: &mut wgpu::CommandEncoder, id: u32) {
        let h = &self.offscreen[&id];
        enc.copy_texture_to_texture(
            h.cur_texture.as_image_copy(),
            h.prev_texture.as_image_copy(),
            wgpu::Extent3d { width: h.width, height: h.height, depth_or_array_layers: 1 },
        );
    }

    /// Resolve a target id to its depth view, created lazily the first time a
    /// depth-flagged pass names that target, sized to the target's current
    /// dimensions. Call only after `target_view(id)` has resolved that same
    /// id this frame, so an offscreen target's size is already known.
    /// `resize()` drops every depth view alongside `offscreen`.
    pub(crate) fn depth_view(&mut self, id: u32) -> &wgpu::TextureView {
        if !self.depth.contains_key(&id) {
            let (width, height) = if id == 0 {
                (self.target.width, self.target.height)
            } else {
                let o = &self.offscreen[&id];
                (o.width, o.height)
            };
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.depth.insert(id, view);
        }
        &self.depth[&id]
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
