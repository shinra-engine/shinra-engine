//! The pure half of the engine: buffers in, buffers out.
//!
//! Nothing in this crate can reach the world, spawn anything, or know what
//! time it is beyond the frame globals it is handed. That is not a convention
//! — `render/*.so` registers nodes and edges and nothing else, so there is no
//! call available to it that could.

pub mod attrs;
pub mod gpu;
pub mod image_asset;
pub mod graph;
pub mod pass;
pub mod mesh;
pub mod prelude_wgsl;
pub mod readback;
pub mod targets;

pub use gpu::Gpu;
pub use graph::{Graph, Scene};
pub use readback::Frame;
#[cfg(not(target_arch = "wasm32"))]
pub use readback::read;
pub use targets::{Target, Targets};

use anyhow::Result;
use se_host::registry::{AssetDef, GraphDef};
use se_host::Space;

/// Everything the render side needs, kept together so the run loop holds one
/// thing rather than four.
pub struct Renderer {
    pub gpu: Gpu,
    pub targets: Targets,
    pub graph: Graph,
    /// How tall a framebuffer pixel is relative to its width, on the surface
    /// that will show it.
    ///
    /// A window has square pixels and this is 1. A terminal does not: a cell
    /// is about twice as tall as it is wide, so a renderer packing 2x4
    /// subpixels into one gets squares, while 2x2 gets pixels twice as tall as
    /// they are wide. Shaders derive aspect from `se.resolution`, so the
    /// resolution they are told has to be the *apparent* one or every scene
    /// drawn in that mode is stretched.
    pub pixel_aspect: f32,
}

impl Renderer {
    /// Stand up a device and build the graph, blocking until the device is
    /// ready. Native only, for the same reason [`Gpu::new`] is: on wasm32 the
    /// wait would be on the event loop that has to make it finish.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(
        buffers: &[se_host::registry::BufferDef],
        graph: &GraphDef,
        screen: (u32, u32),
        space: &dyn Space,
        assets: &[&AssetDef],
    ) -> Result<Renderer> {
        let gpu = Gpu::new()?;
        let targets = Targets::new(&gpu.device, buffers, screen);
        let graph = Graph::build(&gpu.device, &gpu.queue, graph, &targets, &Scene { space, assets })?;
        Ok(Renderer { gpu, targets, graph, pixel_aspect: 1.0 })
    }

    /// Swap in a rebuilt `render/*.so` without touching buffers or the world.
    pub fn swap_graph(&mut self, graph: &GraphDef, space: &dyn Space, assets: &[&AssetDef]) -> Result<()> {
        self.graph =
            Graph::build(&self.gpu.device, &self.gpu.queue, graph, &self.targets, &Scene { space, assets })?;
        Ok(())
    }

    pub fn resize(&mut self, screen: (u32, u32), space: &dyn Space, assets: &[&AssetDef]) -> Result<()> {
        if screen == self.targets.screen() {
            return Ok(());
        }
        self.targets.resize(&self.gpu.device, screen);
        self.graph
            .recompile(&self.gpu.device, &self.gpu.queue, &self.targets, &Scene { space, assets })
    }

    /// Run the graph and read back what it presented.
    ///
    /// Blocking, so native only; [`Renderer::frame_async`] is the wasm32 twin.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn frame(
        &mut self,
        space: &dyn Space,
        assets: &[&AssetDef],
        time: f64,
        dt: f32,
        index: u64,
    ) -> Result<Frame> {
        self.graph.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.targets,
            &Scene { space, assets },
            time,
            dt,
            index,
            self.pixel_aspect,
        )?;
        let present = self.targets.get(&self.graph.def().present)?;
        let frame = readback::read(&self.gpu.device, &self.gpu.queue, present)?;
        self.targets.flip();
        Ok(frame)
    }

    /// The same frame, waited on the event loop instead of on a thread.
    ///
    /// A mirror of [`Renderer::frame`] down to the argument list: the graph is
    /// the same graph, the globals are the same globals, and the only thing
    /// that differs is which of `readback`'s two waits it uses. Native
    /// behaviour cannot move, because native does not go through here.
    #[cfg(target_arch = "wasm32")]
    pub async fn frame_async(
        &mut self,
        space: &dyn Space,
        assets: &[&AssetDef],
        time: f64,
        dt: f32,
        index: u64,
    ) -> Result<Frame> {
        self.graph.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.targets,
            &Scene { space, assets },
            time,
            dt,
            index,
            self.pixel_aspect,
        )?;
        let present = self.targets.get(&self.graph.def().present)?;
        let frame = readback::read_async(&self.gpu.device, &self.gpu.queue, present).await?;
        self.targets.flip();
        Ok(frame)
    }
}
