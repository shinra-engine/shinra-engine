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

use crate::audio::{self, HeadlessSink};
use crate::deck::Deck;
use crate::live::open;
use crate::inspect::{hierarchy, inspect, project_tree, Target};
use crate::pad::{PadSource, ScriptedPad};
use crate::reload::{Outcome, Reloader, Selection};
use crate::render_worker::RenderReply;
use crate::report;
use crate::savefile;
use crate::touch::{keys_of, Touch};
use anyhow::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal as RTerminal;
use se_gpu::command;
use se_tui::{Areas, Ide, Mode, Pixels, TextArtMode, Terminal};
use se_vm::memory::Slot;
use std::path::{Path, PathBuf};
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
    /// A scripted gamepad sequence; `None` means no `pad` control var is ever
    /// written, same as before this existed.
    pub pad: Option<PathBuf>,
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

/// How many fixed steps real time `dt` buys against the carried-over
/// remainder `acc`, and the remainder left after taking them. Capped at 4 so
/// a stall never makes the game try to catch up all at once. Pulled out of
/// the loop so it can be driven by a synthetic `dt` sequence in a test,
/// independent of the render-skip decision below.
fn advance_steps(acc: f32, dt: f32) -> (u32, f32) {
    let acc = acc + dt.min(0.25);
    let steps = ((acc / se_vm::clock::STEP) as u32).min(4);
    (steps, acc - steps as f32 * se_vm::clock::STEP)
}

/// Whether there is enough time left before `TARGET` to pay for a render
/// costing `last_cost`. Computed independently of `advance_steps`: it must
/// never feed back into the simulation's step count or accumulator.
///
/// No longer called from `run()`'s loop — render moved to its own thread, so
/// the loop never has a render cost to budget against; kept for its own
/// test coverage below.
#[allow(dead_code)]
fn should_skip_render(remaining: Option<Duration>, last_cost: Duration) -> bool {
    remaining.is_some_and(|r| r < last_cost)
}

pub fn run(mut deck: Deck, opt: Options) -> Result<()> {
    let mut term = Terminal::enter()?;
    let mut rt = RTerminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let mut ide = Ide { games: deck.names(), current: deck.current, ..Default::default() };

    let size = rt.size()?;
    let mut cells = viewport_guess(size.width, size.height);
    let mut screen = framebuffer_for(cells.0, cells.1, ide.art);

    let mut pad_source = opt.pad.as_deref().map(ScriptedPad::open).transpose()?;
    let mut touch = Touch::new();

    let mut log = Vec::new();
    let mut live = open(&deck, &opt, screen, &mut log)?;
    ide.adapter = live.adapter.clone();
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
    // Holds the last drawn frame so a skipped render redraws it instead of a
    // blank one.
    let mut picture: Vec<u8> = Vec::new();
    // Size `picture` matches, lagging `screen` till a reply arrives for it.
    let mut shown = (0u32, 0u32);
    let mut inflight_screen: Option<(u32, u32)> = None;
    // No real audio device exists to target yet; see audio.rs.
    let mut audio_sink = HeadlessSink::default();

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
            live.render.resize(screen.0, screen.1);
        }

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32().min(0.1);
        last = now;
        ide.fps = if ide.fps == 0.0 { 1.0 / dt } else { ide.fps * 0.9 + (1.0 / dt) * 0.1 };

        // The keyboard is a control var: written before the tick, read by the
        // game at an address the contract fixed, with no host call.
        live.vm
            .set_input(keys_of(&term, &mut touch, term.play, &areas))?;

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
            let (steps, new_acc) = advance_steps(acc, dt);
            acc = new_acc;
            let mut r = Ok((0, 0));
            for _ in 0..steps {
                if let Some(ps) = pad_source.as_mut() {
                    if let Err(e) = live.vm.set_pad(ps.poll(index as u32)) {
                        r = Err(e);
                        break;
                    }
                }
                r = live.vm.frame(index as u32);
                if r.is_err() {
                    break;
                }
                match live.vm.take_sound() {
                    Ok(Some(event)) => {
                        if let Err(w) = audio::dispatch(&live.vm, event, &mut audio_sink) {
                            ide.note(w);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        r = Err(e);
                        break;
                    }
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
        for s in audio_sink.take_log() {
            ide.note(s);
        }
        if let Err(e) = stepped {
            report::frame_error(&mut ide, &e, live.vm.take_rollback());
        }

        // Render runs on its own thread against a point-in-time snapshot, so
        // this loop never waits on it: offer the worker the freshest one and
        // take back whatever picture it last finished. `picture` is left
        // alone whenever nothing new has arrived — same visual result as the
        // old time-budget skip, now arising from the worker still being busy
        // (a full channel) instead of a guess about render cost.
        match command::read(&live.vm.meta, live.vm.bytes()) {
            Ok((cmds, _)) => {
                let sent = live.render.try_send_snapshot(live.vm.meta.clone(), live.vm.world.clone(), live.vm.bytes().to_vec(), cmds);
                inflight_screen = if sent { Some(screen) } else { inflight_screen };
            }
            Err(e) => ide.note(format!("commands: {e:#}")),
        }
        match live.render.try_recv_reply() {
            Some(RenderReply::Picture(p)) => {
                live.cache_render();
                picture = p;
                shown = inflight_screen.take().unwrap_or(shown);
            }
            Some(RenderReply::TimedOut) => handle_render_timeout(&mut live, &mut ide),
            Some(RenderReply::Failed(e)) => {
                handle_render_failure(&mut live, &mut ide, format!("replay: {e}"), format!("{e}"))
            }
            None => {}
        }

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

        let frame = Pixels { width: shown.0, height: shown.1, rgba: &picture };
        rt.draw(|f| areas = se_tui::draw(f, &ide, Some(&frame)))?;

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

/// Any replay failure (timeout or otherwise) rolls the render slot back to
/// the module that last produced a picture (a CPU trap's recovery). `note` is
/// logged as-is when no rollback is possible or rollback itself fails;
/// `headline` becomes the red error line, via `report::render_timeout`, when
/// rollback succeeds.
fn handle_render_failure(live: &mut crate::live::Live, ide: &mut se_tui::Ide, note: String, headline: String) {
    if live.last_good_render.is_none() {
        return ide.note(note);
    }
    match live.rollback_render() {
        Ok(()) => report::render_timeout(ide, &anyhow::anyhow!(headline)),
        Err(rollback_err) => ide.note(format!("{note}; rollback failed: {rollback_err:#}")),
    }
}

/// A GPU timeout rolls the render slot back to the module that last produced
/// a picture; the worker thread has already classified the replay failure as
/// a timeout before this runs, so there is no `anyhow::Error` left to
/// downcast here.
fn handle_render_timeout(live: &mut crate::live::Live, ide: &mut se_tui::Ide) {
    handle_render_failure(live, ide, "replay: GPU timed out".to_string(), "GPU replay timed out".to_string());
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

#[cfg(test)]
mod tests;
