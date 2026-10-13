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

use crate::deck::Deck;
use crate::live::open;
use crate::inspect::{hierarchy, inspect, project_tree, Target};
use crate::reload::{Outcome, Reloader, Selection};
use crate::report;
use crate::savefile;
use anyhow::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal as RTerminal;
use se_gpu::command;
use se_tui::{Areas, Ide, Mode, Pixels, TextArtMode, Terminal};
use se_vm::input::Keys;
use se_vm::memory::Slot;
use std::path::Path;
use std::time::{Duration, Instant};

/// Terminal presentation is cheap but not free, and a loop that spins flat out
/// only makes the terminal the bottleneck.
const TARGET: Duration = Duration::from_millis(33);

/// How often the loop asks the disk whether a source changed. Polling mtimes
/// is deliberate: no watcher dependency, and an edit shows within a second.
const WATCH: Duration = Duration::from_millis(250);

pub struct Options {
    pub profile: String,
    pub frames: Option<u64>,
    pub game: Option<String>,
    pub render: Option<String>,
    /// Poll the source tree and reload what an edit made stale.
    pub watch: bool,
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
            (se_tui::key::SPACE, k::SPACE),
            (se_tui::key::ENTER, k::ENTER),
            (se_tui::key::LEFT, k::LEFT),
            (se_tui::key::RIGHT, k::RIGHT),
            (se_tui::key::UP, k::UP),
            (se_tui::key::DOWN, k::DOWN),
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
    let mut polled = t0;
    // Lives across polls so a broken unit is compiled once, not every poll.
    // A different game is a different tree: it gets a new one.
    let mut reloader: Option<Reloader> = None;
    let mut index = 0u64;
    // Real time not yet spent on simulation steps.
    let mut acc = 0f32;
    let mut areas = Areas::default();
    let mut row_map: Vec<Option<Target>> = Vec::new();
    let mut project_ids: Vec<Option<(String, String)>> = Vec::new();

    loop {
        let started = Instant::now();
        // The router needs the mode before it reads a key: in the editor the
        // IDE owns the keyboard, while playing the game does.
        term.play = ide.mode.is_play();
        term.typing = ide.editing.is_some();
        term.on_inspector = ide.focused == se_tui::Panel::Inspector;
        term.on_project = ide.focused == se_tui::Panel::Project;
        term.poll()?;
        if term.quit {
            break;
        }

        let it = term.intent;
        crate::intents::apply(it, &mut ide, &mut live.vm, &row_map, &areas);

        if it.save {
            match savefile::save(&live.vm, deck.src().name.as_str(), index) {
                Ok(m) => ide.note(m),
                Err(e) => ide.note(format!("save failed: {e:#}")),
            }
        }
        if it.load {
            match savefile::load(&mut live.vm, deck.src().name.as_str()) {
                Ok((f, m)) => {
                    index = f;
                    acc = 0.0;
                    ide.note(m);
                }
                Err(e) => ide.note(format!("load failed: {e:#}")),
            }
        }
        if it.swap {
            crate::swap::swap_selected(&mut ide, &mut live, deck.src(), &project_ids, screen);
        }
        // `n` — the swipe. A different game is a different contract, so
        // nothing is carried over: the old one is dropped whole.
        if (it.next_game || it.prev_game) && deck.step(if it.next_game { 1 } else { -1 }) {
            ide.current = deck.current;
            ide.note(format!("→ {}", ide.games[deck.current]));
            reopen(&deck, &opt, screen, &mut ide, &mut live);
            reloader = None;
            acc = 0.0;
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

        if opt.watch && polled.elapsed() >= WATCH {
            polled = Instant::now();
            if watch(&deck, &opt, screen, &mut ide, &mut live, &mut reloader)
                && !reopen(&deck, &opt, screen, &mut ide, &mut live)
            {
                reloader.as_mut().map(Reloader::restart_failed);
            }
        }

        // No tick while editing: the scene stands still so it can be read. The
        // graph still records and replays, so the picture is live either way.
        // Play advances by whole fixed steps from real time (0..4 per drawn
        // frame), so the simulation rate never depends on the draw time.
        let stepped = if ide.mode.is_play() {
            acc += dt.min(0.25);
            let steps = ((acc / se_vm::clock::STEP) as u32).min(4);
            acc -= steps as f32 * se_vm::clock::STEP;
            let mut r = Ok((0, 0));
            for _ in 0..steps {
                r = live.vm.frame(index as u32);
                if r.is_err() {
                    break;
                }
                index += 1;
            }
            if steps == 0 { live.vm.record_only() } else { r }
        } else {
            live.vm.record_only()
        };
        for o in live.vm.take_overruns() {
            ide.note(o);
        }
        if let Err(e) = stepped {
            report::frame_error(&mut ide, &e, live.vm.take_rollback());
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
        let active: Vec<(String, String)> = [("game", Slot::Game), ("render", Slot::Render)]
            .into_iter()
            .filter_map(|(c, s)| live.vm.module(s).map(|m| (c.to_string(), m.name.clone())))
            .collect();
        let (lines, ids) = project_tree(deck.src(), &active);
        ide.project = lines;
        ide.project_pick = ids.iter().map(|m| m.is_some()).collect();
        project_ids = ids;
        ide.clamp_project();

        let picture = px.unwrap_or_default();
        let shown = Pixels { width: screen.0, height: screen.1, rgba: &picture };
        rt.draw(|f| areas = se_tui::draw(f, &ide, Some(&shown)))?;

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

/// Stand the deck's current entry up again, dropping the old game whole. On
/// failure the running one stays and this returns false.
fn reopen(deck: &Deck, opt: &Options, screen: (u32, u32), ide: &mut Ide, live: &mut crate::live::Live) -> bool {
    let mut l = Vec::new();
    match open(deck, opt, screen, &mut l) {
        Ok(g) => {
            *live = g;
            ide.selected = 0;
            ide.mode = Mode::Edit;
            for m in l {
                ide.note(m);
            }
            true
        }
        Err(e) => {
            ide.note(format!("cannot load {}: {e:#}", ide.games[deck.current]));
            false
        }
    }
}

/// One poll: plan, then swap what is stale. Returns true when the contract
/// moved and the game has to be opened again. The selection is whatever is
/// loaded now, so a swap from the Project panel is watched too.
fn watch(
    deck: &Deck,
    opt: &Options,
    screen: (u32, u32),
    ide: &mut Ide,
    live: &mut crate::live::Live,
    reloader: &mut Option<Reloader>,
) -> bool {
    let src = deck.src();
    let name = |s: Slot| live.vm.module(s).map(|m| m.name.clone());
    let sel = Selection { game: name(Slot::Game), render: name(Slot::Render) };
    let rl = reloader.get_or_insert_with(|| {
        let out = Path::new("target").join("shinra").join(&src.name);
        Reloader::new(src.root.clone(), out, Selection { game: None, render: None }, opt.profile.clone())
    });
    rl.select(sel);
    let started = Instant::now();
    match rl.poll(live, screen) {
        Ok(Outcome::Restart) => {
            ide.note("contract changed: reopening".to_string());
            return true;
        }
        Ok(out) => report::reload(ide, &out, started.elapsed().as_millis()),
        Err(e) => ide.note(format!("reload failed: {e:#}")),
    }
    false
}
