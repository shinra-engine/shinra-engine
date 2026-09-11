//! One game: one world, ticked and rendered.
//!
//! `se-cli`'s `Session` is the shape this follows — it owns the world, the
//! modules behind it and the renderer, and a frame is control, then stages,
//! then the graph. The world is a field of the handle for the same reason it
//! is a field there: a tick and a frame that stand up their own store are
//! about two different worlds, and nothing control writes can reach the graph.
//!
//! A frame is the diagram's three stations in the diagram's order: control
//! decides, the stages update the data, the graph draws it. The middle one is
//! `stage.rs` — on wasm a stage is a named export rather than a pointer filed
//! in a link section, so it is discovered rather than listed.
//!
//! JS holds one of these per game and calls `tick` then `render` on it. That
//! is the reason `renderGame` and `tickGame` are gone rather than kept: they
//! were free functions, and a free function has nowhere to keep a world.

use crate::ctl;
use crate::entry::{err, pair};
use crate::module::{Control, WebModule};
use crate::present;
use crate::render::{self, Loaded, Running};
use crate::stage::{bytes_of, Stages};
use anyhow::{anyhow, ensure, Result};
use se_abi::{Frame, Input};
use se_host::registry::LayoutDef;
use se_host::{Space, Store};
use std::mem::size_of;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

/// Open one game: read every module handed over, and stand up the world they
/// describe.
///
/// The modules are the whole slot — the bundle, one render graph, one asset
/// theme and the control module — because they are ticked and drawn together
/// now. Instantiation stays JS's job; this takes the exports.
#[wasm_bindgen(js_name = openGame)]
pub fn open_game(
    names: js_sys::Array,
    exports: js_sys::Array,
    width: u32,
    height: u32,
) -> Result<Game, JsValue> {
    let (names, exports) = pair(&names, &exports)?;
    Game::open(&names, &exports, (width.max(1), height.max(1))).map_err(err)
}

/// One game's world, the modules behind it, and the device once there is one.
#[wasm_bindgen]
pub struct Game {
    /// `None` only while a tick is running: for the length of a call the world
    /// lives in `ctl`'s registry, because a module reaches it by handle.
    store: Option<Box<dyn Store>>,
    /// The control module and what it registered, or `None` for a set of
    /// modules that has no `game/*.wasm` in it.
    control: Option<(WebModule, Control)>,
    /// Every stage the handed-over modules exported, in the order they run.
    stages: Stages,
    stage: Stage,
    screen: (u32, u32),
    started: bool,
    /// Frames control has run. A render draws the last of them, so the graph's
    /// globals say what control last acted on rather than a time nothing in
    /// the world has seen.
    ran: u64,
}

/// One sixtieth. `game.so` is the only station that knows the time and this
/// route has no clock to hand it yet, so a tick is a fixed step — the same one
/// `shot` uses natively, which is why the two routes' frame `n` agree.
const DT: f32 = 1.0 / 60.0;

/// How far this game is from having a device.
///
/// `start` consumes the bundle to build the graph, so the two states are two
/// values rather than a flag; `Between` exists only while the device is being
/// awaited, which is the one moment neither is true.
enum Stage {
    Cold(Loaded),
    /// Boxed because it is an order of magnitude the larger of the two and a
    /// game is only ever in one of them.
    Live(Box<Live>),
    Between,
}

/// A game with a device under it, and where its frame goes: a swapchain
/// surface, or `None` for the bundles that have to be read back instead.
struct Live {
    run: Running,
    surface: Option<wgpu::Surface<'static>>,
}

impl Stage {
    fn loaded(&self) -> Result<&Loaded> {
        match self {
            Stage::Cold(l) => Ok(l),
            Stage::Live(l) => Ok(l.run.loaded()),
            Stage::Between => Err(anyhow!("this game is still acquiring a device")),
        }
    }
}

#[wasm_bindgen]
impl Game {
    /// Run the game's own control against this world, `n` ticks of it.
    pub fn tick(&mut self, ticks: u32) -> Result<String, JsValue> {
        self.run_ticks(ticks.max(1)).map_err(err)
    }

    /// Draw the world as it now stands onto a canvas.
    ///
    /// The device is acquired on the first call and kept: it belongs to the
    /// game, not to the frame.
    pub async fn render(&mut self, canvas: HtmlCanvasElement) -> Result<String, JsValue> {
        self.draw(&canvas).await.map_err(err)
    }
}

impl Game {
    fn open(names: &[String], exports: &[js_sys::Object], screen: (u32, u32)) -> Result<Game> {
        let loaded = render::collect(names, exports)?;
        // The one place an ECS is named on this route, exactly as on the other.
        let mut store: Box<dyn Store> = Box::new(se_ecs::World::new());
        store.declare(&loaded.layouts);
        // The contract check `Session::stand_up` runs, at the same moment:
        // before a stage has ever been handed a column.
        let stages = crate::stage::collect(names, exports)?;
        stages.validate(&*store)?;
        Ok(Game {
            store: Some(store),
            control: control_of(names, exports)?,
            stages,
            stage: Stage::Cold(loaded),
            screen,
            started: false,
            ran: 0,
        })
    }

    /// `start` once, then `ticks` ticks, against the one world.
    fn run_ticks(&mut self, ticks: u32) -> Result<String> {
        let (m, spec) = self
            .control
            .as_ref()
            .ok_or_else(|| anyhow!("none of these modules registered a control"))?;
        let assets = Rc::clone(&self.stage.loaded()?.assets);

        let mem = m.mem()?;
        // `Ctl` on wasm32 is a single `u32` — the token, not a pointer.
        let ctl_at = mem.alloc(4);
        let input_at = mem.alloc(size_of::<Input>() as u32);
        let frame_at = mem.alloc(size_of::<Frame>() as u32);
        ensure!(
            ctl_at != 0 && input_at != 0 && frame_at != 0,
            "`{}` would not allocate room for the tick's arguments",
            m.name
        );

        let store = self.store.take().ok_or_else(|| anyhow!("the world went missing mid-tick"))?;
        let h = ctl::install(ctl::Host::new(store, assets, m.mem()?));
        let first = (!self.started).then_some(spec.start);
        self.started = true;
        let drove = drive(
            m,
            spec,
            first,
            Args { h, ctl_at, input_at, frame_at, from: self.ran, ticks, screen: self.screen },
            &mem,
            &self.stages,
        );
        // Whatever happened in there, the world comes back out of the
        // registry: a failed tick must not leave this handle without one.
        let host = ctl::reclaim(h);
        mem.free(ctl_at, 4);
        mem.free(input_at, size_of::<Input>() as u32);
        mem.free(frame_at, size_of::<Frame>() as u32);
        let host = host.ok_or_else(|| anyhow!("the world went missing mid-tick"))?;
        self.store = Some(host.store);
        drove?;
        self.ran += ticks as u64;

        let mut out = format!("control `{}` in `{}`, {ticks} ticks\n", spec.name, m.name);
        for (slot, module) in &spec.slots {
            out.push_str(&format!("    binds {slot:?} -> {module}\n"));
        }
        out.push_str(&self.stages.describe());
        for l in &host.req.log {
            out.push_str(&format!("    log: {l}\n"));
        }
        for (slot, module) in &host.req.slots {
            out.push_str(&format!("    asked for {slot:?} -> {module}\n"));
        }
        let store = self.store.as_deref().expect("just put back");
        out.push_str(&world(store, &self.stage.loaded()?.layouts));
        Ok(out)
    }

    /// Which frame a render draws: the last one control produced, or the
    /// still, empty world before any tick has run.
    fn at(&self) -> (f64, f32, u64) {
        match self.ran.checked_sub(1) {
            Some(i) => (i as f64 * DT as f64, DT, i),
            None => (0.0, 0.0, 0),
        }
    }

    /// The frame, on whichever path this bundle's presented buffer asked for.
    async fn draw(&mut self, canvas: &HtmlCanvasElement) -> Result<String> {
        if let Stage::Cold(_) = self.stage {
            // Everything that can be refused is refused before the bundle
            // moves, so a failure here leaves the game exactly as it was.
            let l = self.stage.loaded()?;
            let present = l.buffer(&l.graph()?.present)?.clone();
            let (gpu, surface) = present::acquire(canvas, &present, self.screen).await?;
            let Stage::Cold(loaded) = std::mem::replace(&mut self.stage, Stage::Between) else {
                // Checked just above, and the await cannot have changed it:
                // JS holds this handle borrowed for the whole call, so a
                // second entry into the same game is a `recursive use` throw
                // rather than a second stage transition.
                unreachable!("the stage was Cold and this call owns the handle")
            };
            let world = self.store.as_deref().ok_or_else(|| anyhow!("no world to draw"))?;
            let run = loaded.start(gpu, self.screen, world as &dyn Space)?;
            self.stage = Stage::Live(Box::new(Live { run, surface }));
        }

        let at = self.at();
        let world = self.store.as_deref().ok_or_else(|| anyhow!("no world to draw"))?;
        let Stage::Live(live) = &mut self.stage else {
            return Err(anyhow!("this game has no device under it"));
        };
        let Live { run, surface } = &mut **live;
        let (t, dt, index) = at;
        let name = run.renderer.graph.def().name.clone();
        let adapter = run.renderer.gpu.adapter_name.clone();
        match surface {
            Some(s) => {
                run.run(world as &dyn Space, t, dt, index)?;
                present::to_swapchain(s, &run.renderer.gpu, run.present_target()?)?;
                let (w, h) = run.present_target()?.size;
                run.flip();
                Ok(format!(
                    "graph `{name}` frame {index} straight to the swapchain {w}x{h} \
                     — adapter {adapter}"
                ))
            }
            None => {
                let mut frame = run.frame(world as &dyn Space, t, dt, index).await?;
                present::to_2d(canvas, &mut frame)?;
                Ok(format!(
                    "graph `{name}` frame {index} read back into a 2d context {}x{} \
                     — adapter {adapter}",
                    frame.width, frame.height
                ))
            }
        }
    }
}

/// Where the tick's arguments sit in the module's memory, and which frames to
/// run. Grouped because they are one thing — the shape of one call across the
/// boundary — and because they are all offsets in the same memory.
struct Args {
    h: u32,
    ctl_at: u32,
    input_at: u32,
    frame_at: u32,
    from: u64,
    ticks: u32,
    screen: (u32, u32),
}

/// The calls into the module, with the world already in the registry.
///
/// Separate from `run_ticks` so that every way out of it is a `?` the caller
/// catches *before* reclaiming the world.
fn drive(
    m: &WebModule,
    spec: &Control,
    start: Option<u32>,
    a: Args,
    mem: &crate::module::ModuleMem,
    stages: &Stages,
) -> Result<()> {
    let arg = |v: u32| JsValue::from_f64(v as f64);
    mem.write(a.ctl_at, &a.h.to_le_bytes());
    // The input components are control's to write and nothing on this route
    // reads a keyboard yet, so every tick is handed a zeroed one. The stages
    // are handed the same value, as `&Input` and never more.
    let input = Input::zeroed();
    mem.write(a.input_at, bytes_of(&input));

    // `start` reconciles the roster before any tick — design.md's "a reconcile
    // stage spawns", and the reason a first frame has anything in it at all.
    if let Some(f) = start.filter(|f| *f != 0) {
        m.call(f, &[arg(a.ctl_at)])?;
        ctl::flush(a.h);
    }
    for i in 0..a.ticks as u64 {
        let index = a.from + i;
        let f = Frame {
            t: index as f64 * DT as f64,
            dt: DT,
            index,
            input: a.input_at as *const Input,
            width: a.screen.0,
            height: a.screen.1,
        };
        mem.write(a.frame_at, bytes_of(&f));
        m.call(spec.tick, &[arg(a.ctl_at), arg(a.frame_at)])?;
        ctl::flush(a.h);
        // Then the stages, over the world control has just finished writing —
        // which is still in the registry, because the tick is not over.
        ctl::with_store(a.h, |w| stages.run(w, &input, DT))
            .ok_or_else(|| anyhow!("the world went missing mid-tick"))??;
    }
    // `stop` is not called here. It ends a control's life, and this handle
    // outlives the call: a game is stopped by being dropped, and JS drops it.
    Ok(())
}

/// The first module that registered a control. Native reaches it by path;
/// here the loader hands modules over and the registration says which is which.
fn control_of(
    names: &[String],
    exports: &[js_sys::Object],
) -> Result<Option<(WebModule, Control)>> {
    for (n, e) in names.iter().zip(exports) {
        let m = WebModule::new(n.clone(), e.clone());
        if let Some(c) = m.control()? {
            return Ok(Some((m, c)));
        }
    }
    Ok(None)
}

/// What the world holds now, decoded through the bundle's own layouts.
pub fn world(store: &dyn Store, layouts: &[LayoutDef]) -> String {
    let mut out = String::new();
    let mut seen: Vec<&str> = Vec::new();
    for l in layouts {
        // `data.rs` is linked into every module of a bundle, so the same
        // layout arrives once per module handed over. It is one component.
        if seen.contains(&l.name.as_str()) {
            continue;
        }
        seen.push(&l.name);
        let Some(col) = store.column(&l.name) else { continue };
        if col.is_empty() {
            continue;
        }
        let first = col.bytes().get(..l.size as usize).unwrap_or(&[]);
        out.push_str(&format!("    {} x{} first:{}\n", l.name, col.len(), fields(l, first)));
    }
    out
}

fn fields(l: &LayoutDef, b: &[u8]) -> String {
    use se_abi::ScalarTy;
    let mut s = String::new();
    for f in &l.fields {
        s.push_str(&format!(" {}=", f.name));
        for i in 0..f.count.max(1) {
            let at = (f.offset + i * f.ty.size()) as usize;
            let Some(w) = b.get(at..at + f.ty.size() as usize) else { continue };
            if i > 0 {
                s.push(',');
            }
            s.push_str(&match f.ty {
                ScalarTy::U8 => format!("{}", w[0]),
                ScalarTy::I32 => format!("{}", i32::from_le_bytes(w.try_into().unwrap())),
                ScalarTy::U32 => format!("{}", u32::from_le_bytes(w.try_into().unwrap())),
                ScalarTy::F32 => format!("{:.3}", f32::from_le_bytes(w.try_into().unwrap())),
                ScalarTy::F64 => format!("{:.3}", f64::from_le_bytes(w.try_into().unwrap())),
            });
        }
    }
    s
}
