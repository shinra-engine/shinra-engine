//! The run loop and the IDE around it.
//!
//! Order within a frame is design.md's diagram read left to right: control
//! decides, stages update data, the graph records, the host replays, the
//! terminal presents. Nothing reaches backwards.
//!
//! The unit is a *deck*, not a game. `n` tears the current game down and
//! stands the next one up, which is what gametok means by a swipe. While a
//! game is running the keyboard is its own — `m` is whatever that game decided
//! `m` means — and the host keeps only the keys that leave or switch a run.

use crate::build;
use crate::deck::Deck;
use crate::inspect::{hierarchy, inspect, project_tree, Target};
use anyhow::{bail, Context, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal as RTerminal;
use se_gpu::{command, Gpu};
use se_tui::{Areas, Ide, Mode, Pixels, TextArtMode, Terminal};
use se_vm::input::Keys;
use se_vm::memory::Slot;
use se_vm::Vm;
use std::path::Path;
use std::time::{Duration, Instant};

/// Terminal presentation is cheap but not free, and a loop that spins flat out
/// only makes the terminal the bottleneck.
const TARGET: Duration = Duration::from_millis(33);
/// Rows per pool. One number: a pool is homogeneous by construction, so its
/// columns cannot want different capacities.
const ROWS: u32 = 1024;

pub struct Options {
    pub profile: String,
    pub frames: Option<u64>,
    pub game: Option<String>,
    pub render: Option<String>,
}

/// One game, standing.
struct Live {
    vm: Vm,
    gpu: Gpu,
    /// The column the recorded frame draws, which is what the pipeline was
    /// built for.
    column: u32,
}

fn pick(src: &crate::discover::GameSrc, out: &Path, cat: &str, want: Option<&str>) -> Option<(String, std::path::PathBuf)> {
    let c = src.category(cat)?;
    if build::is_additive(cat) {
        if c.modules.is_empty() {
            return None;
        }
        return Some((cat.to_string(), out.join(cat).join(format!("{cat}.wasm"))));
    }
    let m = match want {
        Some(n) => c.modules.iter().find(|m| m.name == n)?,
        None => c.modules.first()?,
    };
    Some((m.name.clone(), out.join(cat).join(format!("{}.wasm", m.name))))
}

/// Build whichever game the deck points at, stand it up, and make a pipeline
/// for the column it turns out to draw.
fn open(deck: &Deck, opt: &Options, size: (u32, u32), log: &mut Vec<String>) -> Result<Live> {
    let src = deck.src();
    let out = Path::new("target").join("shinra").join(&src.name);
    build::build(src, &out, &opt.profile)?;

    let mut vm = Vm::new(&std::fs::read(out.join("contract.wasm"))?, ROWS)?;
    for (slot, cat, want) in [
        (Slot::Game, "game", opt.game.as_deref()),
        (Slot::Process, "process", None),
        (Slot::Render, "render", opt.render.as_deref()),
    ] {
        let Some((name, path)) = pick(src, &out, cat, want) else {
            bail!("`{}` has no {cat}/*.rs to fill the {cat} slot", src.name)
        };
        vm.load(slot, &name, std::fs::read(&path)?)
            .with_context(|| format!("loading {cat}/{name}.wasm"))?;
    }
    if !vm.stages().is_empty() {
        let n: Vec<&str> = vm.stages().iter().map(|s| s.name.as_str()).collect();
        log.push(format!("stages: {}", n.join(", ")));
    }

    // Assets are data: the bytes ride in a custom section and never need
    // instantiating. A row names one with a `span`, so swapping a 12-byte mesh
    // for an 800-byte one changes two u32s and no layout.
    if let Some(cat) = src.category("asset") {
        for (i, m) in cat.modules.iter().enumerate() {
            let bytes = std::fs::read(out.join("asset").join(format!("{}.wasm", m.name)))?;
            if let Ok(payload) = se_contract::wasm::section(&bytes, "se.asset") {
                vm.put_blob(i as u32, payload)?;
                log.push(format!("asset {}: slot {i}, {} B", m.name, payload.len()));
            }
        }
    }

    // One frame first, so the module says what it wants drawn rather than the
    // host deciding for it.
    vm.frame(0)?;
    let (cmds, _) = command::read(&vm.meta, vm.bytes())?;
    let draw = cmds
        .iter()
        .find(|c| c.kind == command::DRAW_INSTANCED)
        .context("the recorded frame has no instanced draw to build a pipeline for")?;
    let column = draw.column;

    let (rname, rpath) = pick(src, &out, "render", opt.render.as_deref()).expect("checked");
    let render_bytes = std::fs::read(rpath)?;
    let wgsl = std::str::from_utf8(
        se_contract::wasm::section(&render_bytes, "se.wgsl")
            .with_context(|| format!("render/{rname}.wasm carries no se.wgsl section"))?,
    )?
    .to_string();
    let contract_wgsl = std::fs::read_to_string(out.join("contract.wgsl"))?;
    let gpu = Gpu::new(&vm.meta, &format!("{contract_wgsl}\n{wgsl}"), column, size.0, size.1)?;
    Ok(Live { vm, gpu, column })
}

/// The 256-key table the terminal keeps, folded into the three bitmasks the
/// contract declares. Thirty-two keys is every key a game here has bound.
fn keys_of(t: &Terminal) -> Keys {
    use se_vm::input as k;
    let fold = |pick: &dyn Fn(u8) -> bool| -> u32 {
        let mut m = 0u32;
        for c in b'a'..=b'z' {
            if pick(c) || pick(c.to_ascii_uppercase()) {
                m |= k::letter(c);
            }
        }
        for (code, bit) in [
            (se_abi::key::SPACE, k::SPACE),
            (se_abi::key::ENTER, k::ENTER),
            (se_abi::key::LEFT, k::LEFT),
            (se_abi::key::RIGHT, k::RIGHT),
            (se_abi::key::UP, k::UP),
            (se_abi::key::DOWN, k::DOWN),
        ] {
            if pick(code) {
                m |= bit;
            }
        }
        m
    };
    Keys {
        down: fold(&|c| t.input.is_down(c)),
        pressed: fold(&|c| t.input.just_pressed(c)),
        released: fold(&|c| t.input.just_released(c)),
        mouse: t.input.mouse,
        mouse_down: t.input.mouse_down as u32,
    }
}

/// Framebuffer size for a viewport of `cols` x `rows` cells. A cell is not one
/// pixel and not always the same number: Quadrant packs 2x2 subpixels, Braille
/// and Mixed 2x4. Ask the mode rather than assume.
fn framebuffer_for(cols: u16, rows: u16, art: TextArtMode) -> (u32, u32) {
    let (sx, sy) = art.cell_resolution();
    (cols.max(4) as u32 * sx, rows.max(2) as u32 * sy)
}

fn viewport_guess(cols: u16, rows: u16) -> (u16, u16) {
    let w = (cols as u32 * 60 / 100).saturating_sub(2).max(8) as u16;
    let h = ((rows.saturating_sub(1)) as u32 * 70 / 100).saturating_sub(2).max(4) as u16;
    (w, h)
}

pub fn run(mut deck: Deck, opt: Options) -> Result<()> {
    let mut term = Terminal::enter()?;
    let mut rt = RTerminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let mut ide = Ide { games: deck.names(), current: deck.current, ..Default::default() };

    let size = rt.size()?;
    let mut cells = viewport_guess(size.width, size.height);
    let mut screen = framebuffer_for(cells.0, cells.1, ide.art);

    let mut log = Vec::new();
    let mut live = open(&deck, &opt, screen, &mut log)?;
    ide.adapter = live.gpu.adapter().to_string();
    ide.note(format!("gpu: {}", ide.adapter));
    ide.note(format!("deck: {}", ide.games.join(", ")));
    for m in log.drain(..) {
        ide.note(m);
    }

    let t0 = Instant::now();
    let mut last = t0;
    let mut index = 0u64;
    let mut areas = Areas::default();
    let mut row_map: Vec<Option<Target>> = Vec::new();

    loop {
        let started = Instant::now();
        // The router needs the mode before it reads a key: in the editor the
        // IDE owns the keyboard, while playing the game does.
        term.play = ide.mode.is_play();
        term.typing = ide.editing.is_some();
        term.on_inspector = ide.focused == se_tui::Panel::Inspector;
        term.poll()?;
        if term.quit {
            break;
        }

        let it = term.intent;
        crate::intents::apply(it, &mut ide, &mut live.vm, &row_map, &areas);
        // `n` — the swipe. A different game is a different contract, so
        // nothing is carried over: the old one is dropped whole.
        if (it.next_game || it.prev_game) && deck.step(if it.next_game { 1 } else { -1 }) {
            ide.current = deck.current;
            ide.note(format!("→ {}", ide.games[deck.current]));
            let mut l = Vec::new();
            match open(&deck, &opt, screen, &mut l) {
                Ok(g) => {
                    live = g;
                    ide.selected = 0;
                    ide.mode = Mode::Edit;
                    for m in l {
                        ide.note(m);
                    }
                }
                Err(e) => ide.note(format!("cannot load {}: {e:#}", ide.games[deck.current])),
            }
        }

        // Match the framebuffer to the panel that was actually drawn, and to
        // the glyph set in force now — `m` in the editor changes how many
        // subpixels a cell wants, so it resizes the buffer as surely as the
        // window does.
        let size = rt.size()?;
        cells = match areas.viewport_inner() {
            Some((w, h)) if w > 0 && h > 0 => (w, h),
            _ => viewport_guess(size.width, size.height),
        };
        let want = framebuffer_for(cells.0, cells.1, ide.art);
        if want != screen {
            screen = want;
            live.gpu.resize(screen.0, screen.1);
        }

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32().min(0.1);
        last = now;
        ide.fps = if ide.fps == 0.0 { 1.0 / dt } else { ide.fps * 0.9 + (1.0 / dt) * 0.1 };

        // The keyboard is a control var: written before the tick, read by the
        // game at an address the contract fixed, with no host call.
        live.vm.set_input(keys_of(&term))?;

        // No tick while editing: the scene stands still so it can be read. The
        // graph still records and replays, so the picture is live either way.
        let stepped = if ide.mode.is_play() {
            live.vm.frame(index as u32)
        } else {
            live.vm.record_only()
        };
        if let Err(e) = stepped {
            ide.note(format!("frame failed: {e:#}"));
            ide.mode = Mode::Edit;
        }

        let px = match command::read(&live.vm.meta, live.vm.bytes()) {
            Ok((cmds, _)) => {
                match live.gpu.replay(&live.vm.meta, &live.vm.world, live.vm.bytes(), &cmds) {
                    Ok(_) => live.gpu.pixels().ok(),
                    Err(e) => {
                        ide.note(format!("replay: {e:#}"));
                        None
                    }
                }
            }
            Err(e) => {
                ide.note(format!("commands: {e:#}"));
                None
            }
        };

        ide.frame = index;
        ide.hierarchy = hierarchy(&live.vm);
        if ide.selected >= ide.hierarchy.len() {
            ide.selected = 0;
        }
        let (rows, map) = inspect(&live.vm, ide.selected);
        ide.inspector = rows;
        ide.inspector_editable = map.iter().map(|m| m.is_some()).collect();
        row_map = map;
        ide.clamp_inspector();
        ide.project = project_tree(deck.src());

        let picture = px.unwrap_or_default();
        let shown = Pixels { width: screen.0, height: screen.1, rgba: &picture };
        rt.draw(|f| areas = se_tui::draw(f, &ide, Some(&shown)))?;

        index += 1;
        if opt.frames.is_some_and(|n| index >= n) {
            break;
        }
        if let Some(rest) = TARGET.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }

    let log = term.stderr_log().map(|p| p.to_path_buf());
    drop(rt);
    drop(term);
    if let Some(p) = log {
        if std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false) {
            println!("driver output was captured to {}", p.display());
        }
    }
    Ok(())
}
