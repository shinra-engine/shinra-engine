//! The run loop and the IDE around it.
//!
//! Order within a frame is the design's diagram read left to right: control
//! decides, stages update data, the graph turns data into buffers, and the
//! presenter shows one. Nothing reaches backwards.
//!
//! The unit here is a *deck*, not a bundle. `n` tears the current game down
//! and stands the next one up, which is what GameTok means by a swipe.

use crate::build::{self, Engine};
use crate::deck::Deck;
use crate::inspect::{hierarchy, inspect, project_tree, commit_field, RowMap};
use crate::session::Session;
use anyhow::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal as RTerminal;
use se_abi::{Frame, Input};
use se_tui::{Areas, Ide, Mode, Pixels, TextArtMode, Terminal};
use std::time::{Duration, Instant};

/// Terminal presentation is cheap but not free, and a loop that spins flat out
/// just makes the terminal the bottleneck.
const TARGET: Duration = Duration::from_millis(33);

pub struct Options {
    pub profile: String,
    pub watch: bool,
    pub frames: Option<u64>,
    pub game: Option<String>,
}

/// Build and load whichever bundle the deck is pointing at.
fn open(deck: &Deck, engine: &Engine, opt: &Options, screen: (u32, u32)) -> Result<Session> {
    let src = deck.src()?;
    let out = std::path::Path::new("target").join("shinra").join(&src.name);
    build::build(&src, &out, engine, &opt.profile)?;
    Session::open(src, out, screen, opt.game.as_deref())
}

/// The Project panel: the layout rule, as it stands on disk.

pub fn run(mut deck: Deck, engine: &Engine, opt: Options) -> Result<()> {
    let mut row_map: RowMap = Vec::new();
    let mut term = Terminal::enter()?;
    let mut rt = RTerminal::new(CrosstermBackend::new(std::io::stdout()))?;

    let mut ide = Ide { games: deck.names(), current: deck.current, ..Default::default() };
    let size = rt.size()?;
    let mut cells = viewport_cells_guess(size.width, size.height);
    let mut screen = framebuffer_for(cells.0, cells.1, ide.art);
    let mut session = open(&deck, engine, &opt, screen)?;

    ide.adapter = format!(
        "{}{}",
        session.renderer.gpu.adapter_name,
        if session.renderer.gpu.is_software { " (software)" } else { "" }
    );
    ide.note(format!("gpu: {}", ide.adapter));
    ide.note(format!("deck: {}", ide.games.join(", ")));
    session.start();

    let t0 = Instant::now();
    let mut last = t0;
    let mut index = 0u64;
    let mut watch_at = Instant::now();
    let mut areas = Areas::default();

    loop {
        let frame_start = Instant::now();
        // The router needs to know the mode before it reads a key: in the
        // editor the IDE owns the keyboard, while playing the game does.
        term.play = ide.mode.is_play();
        term.typing = ide.editing.is_some();
        term.on_inspector = ide.focused == se_tui::Panel::Inspector;
        term.poll()?;
        if term.quit {
            break;
        }

        // --- IDE keys ------------------------------------------------------
        let it = term.intent;
        if it.cycle_panel {
            ide.focused = ide.focused.next();
        }
        if let Some(p) = it.focus {
            ide.focused = p;
        }
        if let Some((c, r)) = it.click {
            if let Some(p) = areas.panel_at(c, r) {
                ide.focused = p;
            }
        }
        if it.select != 0 {
            ide.move_selection(it.select);
        }
        if let Some(open) = it.expand {
            ide.set_expanded(open);
        }
        if it.inspect_select != 0 {
            ide.move_inspector(it.inspect_select);
        }
        if let Some(c) = it.type_char {
            ide.edit_push(c);
        }
        if it.type_back {
            ide.edit_backspace();
        }
        match it.edit {
            Some(se_tui::Edit::Begin) => {
                if !ide.begin_edit() {
                    ide.note("nothing editable on that row");
                }
            }
            Some(se_tui::Edit::Cancel) => {
                ide.editing = None;
            }
            Some(se_tui::Edit::Commit) => {
                if let Some((row, text)) = ide.take_edit() {
                    match row_map.get(row).and_then(|m| m.clone()) {
                        Some((comp, fi)) => {
                            let sel = ide.selected;
                            match commit_field(&mut session, sel, &comp, fi, &text) {
                                Ok(field) => ide.note(format!("{comp}.{field} ← {text}")),
                                Err(e) => ide.note(format!("edit refused: {e}")),
                            }
                        }
                        None => ide.note("that row holds no value"),
                    }
                }
            }
            None => {}
        }
        if it.cycle_art {
            let name = ide.cycle_art();
            ide.note(format!("viewport: {name}"));
        }
        if it.toggle_run {
            ide.mode = if ide.mode.is_play() { Mode::Edit } else { Mode::Play };
            if ide.mode.is_play() {
                ide.focused = se_tui::Panel::Viewport;
            }
            ide.note(if ide.mode.is_play() { "run" } else { "stopped" });
        }
        if it.leave_run {
            ide.mode = Mode::Edit;
            ide.note("stopped");
        }
        if it.next_game || it.prev_game {
            let delta = if it.next_game { 1 } else { -1 };
            if deck.step(delta) {
                // A different game set is a different contract, so nothing is
                // carried over: the old session is dropped whole.
                ide.current = deck.current;
                ide.note(format!("→ {}", deck.names()[deck.current]));
                match open(&deck, engine, &opt, screen) {
                    Ok(s) => {
                        session = s;
                        session.start();
                        ide.selected = 0;
                        // Switching game lands you in the editor for it, not
                        // mid-play — same as opening one.
                        ide.mode = Mode::Edit;
                    }
                    Err(e) => ide.note(format!("cannot load {}: {e}", ide.game())),
                }
            }
        }

        // Match the framebuffer to the panel the last frame actually drew,
        // and to the glyph set in force now — `m` changes how many subpixels
        // a cell wants, so it resizes the buffer as surely as dragging the
        // window does.
        let size = rt.size()?;
        cells = match areas.viewport_inner() {
            Some((w, h)) if w > 0 && h > 0 => (w, h),
            _ => viewport_cells_guess(size.width, size.height),
        };
        let want = framebuffer_for(cells.0, cells.1, ide.art);
        // A cell is ~2x as tall as wide. Mixed/Braille split it 2x4, giving
        // square subpixels; Quadrant splits it 2x2, giving subpixels twice as
        // tall, so the apparent height is doubled to keep the scene's shape.
        session.renderer.pixel_aspect = 4.0 / ide.art.cell_resolution().1 as f32;
        if want != screen {
            screen = want;
            session.resize(screen)?;
        }

        if opt.watch && (term.reload || watch_at.elapsed() > Duration::from_millis(400)) {
            watch_at = Instant::now();
            match session.reload(engine, &opt.profile, screen) {
                Ok(true) => ide.note("reloaded"),
                Ok(false) => {}
                Err(e) => ide.note(format!("reload failed: {e}")),
            }
        }

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32().min(0.1);
        last = now;
        ide.fps = if ide.fps == 0.0 { 1.0 / dt } else { ide.fps * 0.9 + (1.0 / dt) * 0.1 };

        let input: Input = term.input;
        let frame = Frame {
            t: now.duration_since(t0).as_secs_f64(),
            dt,
            index,
            input: &input as *const Input,
            width: screen.0,
            // The apparent height, so a game deriving aspect from the frame
            // agrees with what its shaders were told.
            height: (screen.1 as f32 * session.renderer.pixel_aspect) as u32,
        };

        // No tick while editing: the scene stands still so it can be read.
        let stepped = if ide.mode.is_play() {
            session.tick(&frame)
        } else {
            session.render_only(&frame)
        };
        let picture = match stepped {
            Ok(p) => p,
            Err(e) => {
                ide.note(format!("frame failed: {e}"));
                break;
            }
        };
        if session.quit {
            break;
        }
        // A game may ask for the next one too — the same swipe, from inside.
        if session.swipe != 0 && deck.step(session.swipe) {
            ide.current = deck.current;
            if let Ok(s) = open(&deck, engine, &opt, screen) {
                session = s;
                session.start();
            }
        }

        for m in session.log.drain(..) {
            ide.note(m);
        }
        ide.frame = index;
        ide.hierarchy = hierarchy(&session);
        if ide.selected >= ide.hierarchy.len() {
            ide.selected = 0;
        }
        let (rows, map) = inspect(&session, ide.selected);
        ide.inspector = rows;
        ide.inspector_editable = map.iter().map(|m| m.is_some()).collect();
        row_map = map;
        ide.clamp_inspector();
        ide.project = project_tree(&session);

        let px = Pixels { width: picture.width, height: picture.height, rgba: &picture.rgba };
        rt.draw(|f| areas = se_tui::draw(f, &ide, Some(&px)))?;

        index += 1;
        if opt.frames.is_some_and(|n| index >= n) {
            break;
        }
        if let Some(rest) = TARGET.checked_sub(frame_start.elapsed()) {
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

/// Framebuffer size for a viewport of `cols` x `rows` character cells.
///
/// A cell is not one pixel and not always the same number of pixels: Quadrant
/// packs 2x2 subpixels into a cell, Braille and Mixed pack 2x4. Ask the mode
/// rather than assume, or the frame fills a corner of the panel and leaves the
/// rest blank — which is exactly what a half-block-sized buffer does when it
/// is handed to a 2x4 renderer.
pub fn framebuffer_for(cols: u16, rows: u16, art: TextArtMode) -> (u32, u32) {
    let (sx, sy) = art.cell_resolution();
    (cols.max(4) as u32 * sx, rows.max(2) as u32 * sy)
}

/// Best guess at the viewport's cell size before the first frame has been
/// drawn and the real rect is known: the middle column of the layout, less
/// its border.
fn viewport_cells_guess(cols: u16, rows: u16) -> (u16, u16) {
    let w = (cols as u32 * 60 / 100).saturating_sub(2).max(8) as u16;
    let h = ((rows.saturating_sub(1)) as u32 * 70 / 100).saturating_sub(2).max(4) as u16;
    (w, h)
}

/// Render a fixed number of frames with no terminal, and report the last one.
/// This is what makes the engine testable without a tty.
pub fn headless(mut s: Session, frames: u64) -> Result<se_render::Frame> {
    s.start();
    let input = Input::zeroed();
    let t0 = Instant::now();
    let mut out = None;
    for index in 0..frames.max(1) {
        let frame = Frame {
            t: t0.elapsed().as_secs_f64(),
            dt: 1.0 / 60.0,
            index,
            input: &input as *const Input,
            width: s.renderer.targets.screen().0,
            height: s.renderer.targets.screen().1,
        };
        out = Some(s.tick(&frame)?);
    }
    Ok(out.expect("at least one frame"))
}
