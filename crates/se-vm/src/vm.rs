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

use crate::clock;
use crate::mem::native::NativeMem;
use crate::memory::{required_pages, Slot};
use crate::module::{self, with_budget, Loaded};
use crate::rollback::Staged;
use crate::save;
use crate::stages::{self, Stage};
use crate::world::{Meta, World, SPAN_BYTES};
use anyhow::{bail, Context, Result};
use se_contract::read::Table;
use wasmtime::{Config, Engine, Memory, MemoryType, Store};

pub struct Vm {
    pub engine: Engine,
    pub store: Store<()>,
    pub memory: Memory,
    pub meta: Meta,
    pub world: World,
    pub(crate) game: Option<Loaded>,
    pub(crate) process: Option<Loaded>,
    pub(crate) render: Option<Loaded>,
    /// The stages the current process module declared, in the order it listed
    /// them. Order is a declaration, not a consequence of filenames, so no
    /// path sorting is needed to settle it.
    pub(crate) stages: Vec<Stage>,
    /// Swaps the next frame has yet to confirm; see `rollback.rs`.
    pub(crate) staged: Vec<Staged>,
    /// The slots the last failed frame put back, for the host to report.
    pub(crate) rolled_back: Option<Vec<Slot>>,
    /// Fingerprint of the contract this VM was built from; a save carries it.
    contract_id: u64,
    /// Roster requests the host had to clamp, for the UI to show.
    overruns: Vec<String>,
}

impl Vm {
    /// Stand up a memory and place the world the contract describes.
    pub fn new(contract_wasm: &[u8], rows: u32) -> Result<Vm> {
        let table_bytes = se_contract::wasm::section(contract_wasm, se_contract::table::SECTION)
            .context("contract.wasm carries no se.contract section")?;
        let contract_id = save::fingerprint(table_bytes);
        let table = Table::new(table_bytes)?;
        let meta = Meta::read(&table)?;

        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config)?;
        let mut store = Store::new(&engine, ());
        let pages = required_pages();
        let memory = Memory::new(&mut store, MemoryType::new(pages, Some(pages)))
            .with_context(|| format!("reserving {pages} pages"))?;

        let world = {
            let mut mem = NativeMem { memory, store: &mut store };
            World::place(&meta, &mut mem, rows)?
        };
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
            staged: Vec::new(),
            rolled_back: None,
            contract_id,
            overruns: Vec::new(),
        })
    }

    /// The fingerprint of the contract this world was placed from.
    pub fn contract_id(&self) -> u64 { self.contract_id }

    /// Refuse bytes built against another contract (for assets, never loaded).
    pub fn check_contract(&self, name: &str, bytes: &[u8]) -> Result<()> {
        module::check_contract_id(name, bytes, self.contract_id)
    }

    /// Put a module in a slot, replacing whatever was there.
    ///
    /// Replacing is the whole point and costs nothing here: the new module's
    /// statics land on the same `--global-base` as the old one's, and the
    /// world is not in either of them.
    pub fn load(&mut self, slot: Slot, name: &str, bytes: Vec<u8>) -> Result<()> {
        let p = Loaded::prepare(&self.engine, slot, name, bytes, self.contract_id)?;
        if slot != Slot::Process {
            let m = Loaded::commit(p, &mut self.store, self.memory)?;
            let prev = self.slot_mut(slot).replace(m);
            self.stage(slot, prev, Vec::new());
            return Ok(());
        }
        // Discovered, never listed: the host reads what the module declared
        // and checks it against the contract, so a stage written after this
        // host was built still loads. All of it is read before instantiation;
        // a refusal after `commit` would have overwritten the running statics.
        let mut declared = match p.section(stages::SECTION) {
            Ok(bytes) => stages::decode(bytes)
                .with_context(|| format!("the stage table in `{}`", p.name))?,
            // No table, no stages: right for a control module or an asset.
            Err(_) => Vec::new(),
        };
        stages::resolve(&mut declared, &self.meta)?;
        for s in &declared {
            if !p.has_export(&s.export) {
                bail!("`{}` declares stage `{}` but exports no {}", p.name, s.name, s.export)
            }
        }
        let m = Loaded::commit(p, &mut self.store, self.memory)?;
        let prev_stages = std::mem::replace(&mut self.stages, declared);
        let prev = self.process.replace(m);
        self.stage(slot, prev, prev_stages);
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
        let what = format!("`{name}` {sym}");
        match arg {
            Some(a) => {
                let f = inst
                    .get_typed_func::<u32, u32>(&mut self.store, sym)
                    .with_context(|| format!("`{name}` exports no {sym}(u32)"))?;
                with_budget(&mut self.store, &what, |s| f.call(s, a).map_err(Into::into))
            }
            None => {
                let f = inst
                    .get_typed_func::<(), u32>(&mut self.store, sym)
                    .with_context(|| format!("`{name}` exports no {sym}()"))?;
                with_budget(&mut self.store, &what, |s| f.call(s, ()).map_err(Into::into))
            }
        }
    }

    /// One frame. Returns how many rows the pipeline touched and how many
    /// commands were recorded, which is all the host needs to know.
    ///
    /// The first frame after a swap confirms it; one that fails puts the
    /// previous module back (`rollback.rs`).
    pub fn frame(&mut self, n: u32) -> Result<(u32, u32)> {
        let r = self.run_frame(n);
        self.settle(r, &Slot::ALL)
    }

    fn run_frame(&mut self, n: u32) -> Result<(u32, u32)> {
        self.set_clock(clock::Clock::at(n))?;
        self.call(Slot::Game, "tick", Some(n)).context("game.wasm tick")?;

        // Reconcile. A control module asks for a roster by writing a pool's
        // length; the host is what keeps that honest against the capacity it
        // placed. This is the whole of what used to be spawn and despawn.
        for p in 0..self.world.pool_cap.len() as u32 {
            let asked = {
                let mem = NativeMem { memory: self.memory, store: &mut self.store };
                self.world.len_of(&self.meta, &mem, p)?
            };
            let cap = self.world.pool_cap[p as usize];
            if asked > cap {
                self.set_len(p, cap)?;
                let name = &self.meta.pools[p as usize].name;
                let msg = format!("game asked pool `{name}` for {asked} rows; capacity is {cap}");
                if self.overruns.last() != Some(&msg) {
                    self.overruns.push(msg);
                }
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

    /// The clamps `frame` applied since the last call.
    pub fn take_overruns(&mut self) -> Vec<String> {
        std::mem::take(&mut self.overruns)
    }

    /// Copy an asset's bytes into the blob region and publish its span.
    /// Variable length lives there; the record that names it does not grow.
    pub fn put_blob(&mut self, slot: u32, payload: &[u8]) -> Result<(u32, u32)> {
        let mut mem = NativeMem { memory: self.memory, store: &mut self.store };
        self.world.put_blob(&self.meta, &mut mem, slot, payload)
    }

    pub fn put_str(&mut self, slot: u32, s: &str) -> Result<(u32, u32)> {
        self.put_blob(slot, s.as_bytes())
    }

    pub fn put_u32s(&mut self, slot: u32, v: &[u32]) -> Result<(u32, u32)> {
        let b: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.put_blob(slot, &b)
    }

    pub fn put_f32s(&mut self, slot: u32, v: &[f32]) -> Result<(u32, u32)> {
        let b: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.put_blob(slot, &b)
    }

    /// The span of a blob slot, as published in the arena.
    fn span(&self, slot: u32) -> Result<[u8; 8]> {
        if slot >= self.meta.blob_slots {
            bail!("blob slot {slot} is past the {} the contract declares", self.meta.blob_slots)
        }
        let v = self.read(self.meta.blobs_at + slot * SPAN_BYTES, SPAN_BYTES)?;
        Ok(v.try_into().expect("read returns the length asked"))
    }

    /// The bytes a blob slot's span names.
    pub fn blob(&self, slot: u32) -> Result<Vec<u8>> {
        let s = self.span(slot)?;
        let off = u32::from_le_bytes(s[..4].try_into().unwrap());
        let len = u32::from_le_bytes(s[4..].try_into().unwrap());
        self.read(off, len)
    }

    /// Copy a slot's span into a record field at arena address `at`.
    pub fn set_span_field(&mut self, at: u32, slot: u32) -> Result<()> {
        let s = self.span(slot)?;
        self.write(at, &s)
    }

    /// Ask for a pool to hold `len` rows. Nothing spawns: a control var asks,
    /// and the host answers by moving one number.
    pub fn set_len(&mut self, pool: u32, len: u32) -> Result<()> {
        let mut mem = NativeMem { memory: self.memory, store: &mut self.store };
        self.world.set_len(&self.meta, &mut mem, pool, len)
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

    /// A save file: header plus the arena. `frame` is the host's counter,
    /// which is not in the arena.
    pub fn save(&self, frame: u32) -> Vec<u8> {
        save::encode(self.contract_id, frame, self.meta.base, &self.snapshot())
    }

    /// Load a save made by `save`. Everything is checked before memory is
    /// touched, so a refusal leaves the world as it was. Returns the frame.
    pub fn load_save(&mut self, bytes: &[u8]) -> Result<u32> {
        let (h, payload) = save::decode(bytes)?;
        if h.id != self.contract_id {
            bail!("save is from a different game: its contract differs from this one's")
        }
        if h.base != self.meta.base {
            bail!("save arena base 0x{:x} differs from this contract's 0x{:x}", h.base, self.meta.base)
        }
        let end = h.base as u64 + h.len as u64;
        if end > crate::memory::heap_limit() as u64 {
            bail!("save reaches 0x{end:x}, past the heap limit 0x{:x}", crate::memory::heap_limit())
        }
        self.restore(payload)?;
        self.world.blob_top = end as u32;
        for p in 0..self.world.pool_len.len() as u32 {
            let mem = NativeMem { memory: self.memory, store: &mut self.store };
            self.world.pool_len[p as usize] = self.world.len_of(&self.meta, &mem, p)?;
        }
        Ok(h.frame)
    }

    /// Record a frame without advancing one.
    ///
    /// The editor stands the scene still so it can be read, but the picture
    /// has to stay live — a paused game you cannot see is not paused, it is
    /// off. So the graph still records and the host still replays; only
    /// control and the pipeline are skipped.
    ///
    /// Only the render module runs, so only a render swap is confirmed here.
    pub fn record_only(&mut self) -> Result<(u32, u32)> {
        let r = self.call(Slot::Render, "record", None).context("render.wasm record");
        Ok((0, self.settle(r, &[Slot::Render])?))
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
