//! Running a bundle's render graph on this route.
//!
//! Nothing below is route-specific. `Targets` come from the bundle's
//! `buffer.rs`, the `Graph` and its WGSL come from `render/*.rs`, and both are
//! the same `se-render` types the `.so` host builds — the only thing that
//! differs is that `module.rs` read the descriptions out of instantiated wasm
//! instead of out of a dlopened image.
//!
//! The world is not here either. It belongs to `game.rs`, which owns the one
//! the control module ticks, and it arrives as a `&dyn Space` per frame — the
//! same argument `se-cli`'s `Session` hands its renderer. Nothing in this file
//! may create a world: a second world is the bug this route had.

use crate::module::WebModule;
use crate::present::{choose, Path};
use anyhow::{anyhow, bail, Context, Result};
use js_sys::Object;
use se_host::registry::{AssetDef, BufferDef, GraphDef, LayoutDef};
use se_host::Space;
use se_render::{Gpu, Graph, Renderer, Scene, Targets};
use std::rc::Rc;

/// What one game's modules registered, merged into one bundle.
pub struct Loaded {
    pub layouts: Vec<LayoutDef>,
    pub buffers: Vec<BufferDef>,
    pub graphs: Vec<GraphDef>,
    /// Shared rather than owned: a tick lends the same bytes to the control
    /// module (`ctl::Host::lend_asset`) that the graph draws from, and an
    /// asset is model data — copying it per tick would be the largest thing
    /// this route does every frame.
    pub assets: Rc<Vec<AssetDef>>,
}

/// Read every module handed over, the same way `describeModule` reads one.
pub fn collect(names: &[String], exports: &[Object]) -> Result<Loaded> {
    let (mut layouts, mut buffers, mut graphs, mut assets) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (name, ex) in names.iter().zip(exports) {
        let r = WebModule::new(name.clone(), ex.clone())
            .read()
            .with_context(|| format!("module `{name}`"))?;
        layouts.extend(r.layouts);
        buffers.extend(r.buffers);
        graphs.extend(r.graphs);
        assets.extend(r.assets);
    }
    Ok(Loaded { layouts, buffers, graphs, assets: Rc::new(assets) })
}

impl Loaded {
    /// The one graph to run.
    ///
    /// Native reaches it by binding the render slot from `game.so`. Here the
    /// control's `set_slot` is recorded but not yet acted on, so the choice is
    /// still made by which modules the loader hands over — but the rule is the
    /// native rule, because it is the same rule: a slot holds one graph.
    pub fn graph(&self) -> Result<&GraphDef> {
        match self.graphs.as_slice() {
            [one] => Ok(one),
            [] => bail!("none of these modules registered a render graph"),
            many => {
                let names: Vec<&str> = many.iter().map(|g| g.name.as_str()).collect();
                bail!(
                    "{} render graphs were handed over — a slot holds one graph ({})",
                    many.len(),
                    names.join(", ")
                )
            }
        }
    }

    pub fn buffer(&self, name: &str) -> Result<&BufferDef> {
        self.buffers
            .iter()
            .find(|b| b.name == name)
            .ok_or_else(|| anyhow!("no buffer named `{name}` in this bundle"))
    }

    /// A frame described as far as it can be without a device.
    ///
    /// Everything a `Graph::build` would reject for naming a buffer that does
    /// not exist is rejected here too; what is left over is the part that
    /// needs an adapter. That split is what makes this callable where there is
    /// no GPU at all — node, for one — and it is the only honest check
    /// available outside a browser.
    pub fn plan(&self, screen: (u32, u32)) -> Result<String> {
        let g = self.graph()?;
        let p = self.buffer(&g.present)?;
        let (w, h) = p.resolve(screen);
        let via = match choose(p, screen) {
            Path::Surface => "the swapchain",
            Path::Blit => "a 2d blit",
        };
        let mut out = format!(
            "{} layouts, {} buffers, {} assets\ngraph `{}` presents `{}` {:?} {w}x{h} via {via}\n",
            self.layouts.len(),
            self.buffers.len(),
            self.assets.len(),
            g.name,
            p.name,
            p.format,
        );
        for pass in &g.passes {
            for b in pass.color.iter().chain(pass.depth.iter()).chain(pass.reads.iter()) {
                self.buffer(b).with_context(|| format!("pass `{}`", pass.name))?;
            }
            out.push_str(&format!(
                "    pass {} -> {:?} depth={:?} reads={:?} draw={:?} shader={}B\n",
                pass.name,
                pass.color,
                pass.depth,
                pass.reads,
                pass.draw.kind,
                pass.shader.len()
            ));
        }
        for (a, b) in &g.edges {
            let known = |n: &String| g.passes.iter().any(|p| p.name == *n);
            if !known(a) || !known(b) {
                bail!("edge `{a}` -> `{b}` names a pass this graph does not declare");
            }
            out.push_str(&format!("    edge {a} -> {b}\n"));
        }
        Ok(out)
    }

    /// Stand up the targets and the graph on an acquired device, against the
    /// world that is already there.
    ///
    /// This is `se_cli::session::stand_up` minus the halves this route has no
    /// transport for: no stage validation, because no stage can be run. The
    /// world is a parameter for the reason the whole ticket exists — the graph
    /// is built against the one `game.rs` owns and control ticks.
    pub fn start(self, gpu: Gpu, screen: (u32, u32), space: &dyn Space) -> Result<Running> {
        let targets = Targets::new(&gpu.device, &self.buffers, screen);
        let graph = {
            let assets: Vec<&AssetDef> = self.assets.iter().collect();
            let scene = Scene { space, assets: &assets };
            Graph::build(&gpu.device, &gpu.queue, self.graph()?, &targets, &scene)?
        };
        let renderer = Renderer { gpu, targets, graph, pixel_aspect: 1.0 };
        Ok(Running { loaded: self, renderer })
    }
}

/// A bundle with a device under it.
pub struct Running {
    loaded: Loaded,
    pub renderer: Renderer,
}

impl Running {
    /// Encode and submit the graph, and stop there.
    ///
    /// The surface path needs this rather than `frame_async`: it has somewhere
    /// to copy the presented texture to and no reason to bring the pixels back
    /// across to the CPU first.
    pub fn run(&mut self, space: &dyn Space, time: f64, dt: f32, index: u64) -> Result<()> {
        let assets: Vec<&AssetDef> = self.loaded.assets.iter().collect();
        let r = &mut self.renderer;
        r.graph.render(
            &r.gpu.device,
            &r.gpu.queue,
            &r.targets,
            &Scene { space, assets: &assets },
            time,
            dt,
            index,
            r.pixel_aspect,
        )
    }

    /// Run the graph and bring the presented buffer back to the CPU.
    pub async fn frame(
        &mut self,
        space: &dyn Space,
        time: f64,
        dt: f32,
        index: u64,
    ) -> Result<se_render::Frame> {
        let assets: Vec<&AssetDef> = self.loaded.assets.iter().collect();
        self.renderer.frame_async(space, &assets, time, dt, index).await
    }

    /// What the modules registered, for the handle that owns this.
    pub fn loaded(&self) -> &Loaded {
        &self.loaded
    }

    /// The target the graph declared as its output.
    pub fn present_target(&self) -> Result<&se_render::Target> {
        self.renderer.targets.get(&self.renderer.graph.def().present)
    }

    /// End of frame: what was written becomes what can be read. `frame_async`
    /// does this itself; the surface path has to say so.
    pub fn flip(&mut self) {
        self.renderer.targets.flip();
    }
}
