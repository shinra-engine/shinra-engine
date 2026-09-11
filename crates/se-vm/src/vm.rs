//! One game, running.
//!
//! The frame is three calls in a fixed order, and the order is the design:
//! control decides, the data pipeline updates, the graph records.
//!
//! ```text
//! game.wasm      tick()    writes the control var set, and nothing else
//! process.wasm   run()     reads control vars and columns, writes columns
//! render.wasm    record()  writes a command list the host replays
//! ```
//!
//! None of the three takes an argument or returns anything the host acts on.
//! Everything they read and everything they write is in the arena, at
//! addresses the contract fixed — which is why a module can be replaced
//! between any two frames.

use crate::input::{self, Keys};
use crate::memory::{required_pages, Slot};
use crate::module::Loaded;
use crate::stages::{self, Stage};
use crate::world::{Meta, World};
use anyhow::{bail, Context, Result};
use se_contract::read::Table;
use wasmtime::{Engine, Memory, MemoryType, Store};

pub struct Vm {
    pub engine: Engine,
    pub store: Store<()>,
    pub memory: Memory,
    pub meta: Meta,
    pub world: World,
    game: Option<Loaded>,
    process: Option<Loaded>,
    render: Option<Loaded>,
    /// The stages the current process module declared, in the order it listed
    /// them. Order is a declaration, not a consequence of filenames — which is
    /// what `se-web/src/stage.rs:113` had to settle by sorting paths.
    stages: Vec<Stage>,
}

impl Vm {
    /// Stand up a memory and place the world the contract describes.
    pub fn new(contract_wasm: &[u8], rows: u32) -> Result<Vm> {
        let table_bytes = se_contract::wasm::section(contract_wasm, se_contract::table::SECTION)
            .context("contract.wasm carries no se.contract section")?;
        let table = Table::new(table_bytes)?;
        let meta = Meta::read(&table)?;

        let engine = Engine::default();
        let mut store = Store::new(&engine, ());
        let pages = required_pages();
        let memory = Memory::new(&mut store, MemoryType::new(pages, Some(pages)))
            .with_context(|| format!("reserving {pages} pages"))?;

        let world = World::place(&meta, &memory, &mut store, rows)?;
        Ok(Vm {
            engine,
            store,
            memory,
            meta,
            world,
            game: None,
            process: None,
            render: None,
            stages: Vec::new(),
        })
    }

    /// Put a module in a slot, replacing whatever was there.
    ///
    /// Replacing is the whole point and costs nothing here: the new module's
    /// statics land on the same `--global-base` as the old one's, and the
    /// world is not in either of them.
    pub fn load(&mut self, slot: Slot, name: &str, bytes: Vec<u8>) -> Result<()> {
        let m = Loaded::open(&self.engine, &mut self.store, self.memory, slot, name, bytes)?;
        match slot {
            Slot::Game => self.game = Some(m),
            Slot::Process => {
                // Discovered, never listed. The host knows no stage by name;
                // it reads what the module declared and checks it against the
                // contract, which is what lets a stage written long after this
                // host was built still load.
                let mut declared = match m.section(stages::SECTION) {
                    Ok(bytes) => stages::decode(bytes)
                        .with_context(|| format!("the stage table in `{}`", m.name))?,
                    // A module with no table has no stages. That is the right
                    // answer for a control module or an asset, so it is not an
                    // error here either.
                    Err(_) => Vec::new(),
                };
                stages::resolve(&mut declared, &self.meta)?;
                for s in &declared {
                    if !m.has(&mut self.store, &s.export) {
                        bail!(
                            "`{}` declares stage `{}` but exports no {}",
                            m.name,
                            s.name,
                            s.export
                        )
                    }
                }
                self.stages = declared;
                self.process = Some(m);
            }
            Slot::Render => self.render = Some(m),
        }
        Ok(())
    }

    /// What the current process module brought.
    pub fn stages(&self) -> &[Stage] {
        &self.stages
    }

    pub fn module(&self, slot: Slot) -> Option<&Loaded> {
        match slot {
            Slot::Game => self.game.as_ref(),
            Slot::Process => self.process.as_ref(),
            Slot::Render => self.render.as_ref(),
        }
    }

    /// An empty slot is not an error: a game with no render module still
    /// ticks, which is what makes the modules independently replaceable.
    fn call(&mut self, slot: Slot, sym: &str, arg: Option<u32>) -> Result<u32> {
        // `Instance` is `Copy`, so taking it ends the borrow of `self` before
        // `self.store` is borrowed mutably.
        let Some((inst, name)) = self.module(slot).map(|m| (m.instance, m.name.clone())) else {
            return Ok(0);
        };
        Ok(match arg {
            Some(a) => inst
                .get_typed_func::<u32, u32>(&mut self.store, sym)
                .with_context(|| format!("`{name}` exports no {sym}(u32)"))?
                .call(&mut self.store, a)?,
            None => inst
                .get_typed_func::<(), u32>(&mut self.store, sym)
                .with_context(|| format!("`{name}` exports no {sym}()"))?
                .call(&mut self.store, ())?,
        })
    }

    /// One frame. Returns how many rows the pipeline touched and how many
    /// commands were recorded, which is all the host needs to know.
    pub fn frame(&mut self, n: u32) -> Result<(u32, u32)> {
        self.call(Slot::Game, "tick", Some(n)).context("game.wasm tick")?;

        // Reconcile. A control module asks for a roster by writing a pool's
        // length; the host is what keeps that honest against the capacity it
        // placed. This is the whole of what used to be spawn and despawn.
        for p in 0..self.world.pool_cap.len() as u32 {
            let asked = self.world.len_of(&self.meta, &self.memory, &self.store, p)?;
            let cap = self.world.pool_cap[p as usize];
            if asked > cap {
                self.set_len(p, cap)?;
            } else {
                self.world.pool_len[p as usize] = asked;
            }
        }

        // Every stage the module declared, in its declared order. Each is one
        // crossing: the per-row loop runs inside the module, over the host's
        // own columns, with nothing copied either way.
        let calls: Vec<(String, String)> = self
            .stages
            .iter()
            .map(|s| (s.name.clone(), s.export.clone()))
            .collect();
        let mut rows = 0;
        for (name, export) in calls {
            rows = self
                .call(Slot::Process, &export, None)
                .with_context(|| format!("stage `{name}`"))?;
        }

        let cmds = self.call(Slot::Render, "record", None).context("render.wasm record")?;
        Ok((rows, cmds))
    }

    /// Hand the keyboard to the game.
    ///
    /// Checked against the contract the first time, field by field: a game
    /// that spells `input` differently would otherwise be fed three bitmasks
    /// at the wrong offsets and see keys nobody pressed.
    pub fn set_input(&mut self, keys: Keys) -> Result<()> {
        let Some((at, len)) = self.meta.control_at("input") else {
            // A game that declares no `input` simply cannot be typed at. That
            // is a choice its contract made, not an error here.
            return Ok(());
        };
        if len != input::BYTES {
            bail!("`control.input` is {len} bytes; this engine writes {}", input::BYTES)
        }
        let r = self
            .meta
            .record("input")
            .context("the contract names an `input` control var but declares no record")?;
        for (name, off) in input::FIELDS {
            let f = r
                .fields
                .iter()
                .find(|f| f.name.replace('-', "_") == name)
                .with_context(|| format!("`control.input` has no `{name}`"))?;
            if f.offset != off {
                bail!("`control.input.{name}` is at {}; this engine writes {off}", f.offset)
            }
        }
        self.write(at, &keys.to_bytes())
    }

    /// Copy an asset's bytes into the blob region and publish its span.
    /// Variable length lives there; the record that names it does not grow.
    pub fn put_blob(&mut self, slot: u32, payload: &[u8]) -> Result<(u32, u32)> {
        self.world.put_blob(&self.meta, &self.memory, &mut self.store, slot, payload)
    }

    /// Ask for a pool to hold `len` rows. Nothing spawns: a control var asks,
    /// and the host answers by moving one number.
    pub fn set_len(&mut self, pool: u32, len: u32) -> Result<()> {
        self.world.set_len(&self.meta, &self.memory, &mut self.store, pool, len)
    }

    /// The whole arena, byte for byte. A save file is this and nothing else —
    /// there is no entity table to serialise, because there are no entities.
    pub fn snapshot(&self) -> Vec<u8> {
        let end = self.world.blob_top as usize;
        self.memory.data(&self.store)[self.meta.base as usize..end].to_vec()
    }

    /// Put one back. The world resumes; the modules never knew.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<()> {
        self.write(self.meta.base, bytes)
    }

    /// Record a frame without advancing one.
    ///
    /// The editor stands the scene still so it can be read, but the picture
    /// has to stay live — a paused game you cannot see is not paused, it is
    /// off. So the graph still records and the host still replays; only
    /// control and the pipeline are skipped.
    pub fn record_only(&mut self) -> Result<(u32, u32)> {
        let cmds = self.call(Slot::Render, "record", None).context("render.wasm record")?;
        Ok((0, cmds))
    }

    pub fn read(&self, at: u32, len: u32) -> Result<Vec<u8>> {
        let mut v = vec![0u8; len as usize];
        self.memory.read(&self.store, at as usize, &mut v)?;
        Ok(v)
    }

    pub fn write(&mut self, at: u32, bytes: &[u8]) -> Result<()> {
        self.memory.write(&mut self.store, at as usize, bytes)?;
        Ok(())
    }

    /// The whole of linear memory, for handing a column straight to the GPU.
    pub fn bytes(&self) -> &[u8] {
        self.memory.data(&self.store)
    }
}
