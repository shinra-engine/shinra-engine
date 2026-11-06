//! Turns the terminal's raw pointer into the viewport-local, Play-mode-gated
//! form `Keys.mouse` / `Keys.mouse_down` need, and assembles the rest of the
//! frame's `Keys` alongside it. No contract or se-vm change: the existing
//! 24-byte `input` record is used exactly as declared.

use anyhow::{bail, Context, Result};
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use se_tui::touch::Pointer;
use se_tui::{Areas, Panel, Terminal};
use se_vm::input::Keys;
use std::path::Path;

/// Maps a terminal cell to normalized viewport coordinates — 0..1, origin
/// top-left, y down — from the cell's center relative to the viewport's
/// inner rect (border excluded). `None` when the cell falls outside it.
pub fn normalize(origin: (u16, u16), inner: (u16, u16), col: u16, row: u16) -> Option<(f32, f32)> {
    let (w, h) = inner;
    if w == 0 || h == 0 || col < origin.0 || row < origin.1 {
        return None;
    }
    let (lx, ly) = (col - origin.0, row - origin.1);
    if lx >= w || ly >= h {
        return None;
    }
    Some(((lx as f32 + 0.5) / w as f32, (ly as f32 + 0.5) / h as f32))
}

/// Latches whether the currently-held press started inside the viewport
/// during Play mode. "Started inside" gates `mouse_down`, not "currently
/// inside": a press that begins inside and drags out stays down for the rest
/// of that press, and one that begins outside and drags in stays up, until
/// release. Position (`mouse-x`/`mouse-y`) only updates while the pointer is
/// over the viewport; outside it, the last position is kept rather than
/// extrapolated.
#[derive(Default)]
pub struct Touch {
    pos: (f32, f32),
    was_held: bool,
    started_inside: bool,
}

impl Touch {
    pub fn new() -> Touch {
        Touch::default()
    }

    /// Recompute from this frame's pointer state. `play` is whether Play
    /// mode owns the viewport; in Edit mode `mouse_down` is always 0 — clicks
    /// there are the IDE's, via `Intent::click`, and untouched by this.
    /// `areas` has no viewport rect before the first draw.
    pub fn update(&mut self, play: bool, areas: &Areas, pointer: &Pointer) -> ([f32; 2], u32) {
        let (col, row) = pointer.pos();
        let hit = areas
            .viewport_origin()
            .zip(areas.viewport_inner())
            .and_then(|(o, i)| normalize(o, i, col, row));
        if let Some(p) = hit {
            self.pos = p;
        }
        let held = pointer.held();
        if held && !self.was_held {
            self.started_inside = play && hit.is_some();
        }
        self.was_held = held;
        let mouse_down = (held && self.started_inside) as u32;
        ([self.pos.0, self.pos.1], mouse_down)
    }
}

/// The 256-key table the terminal keeps, plus the touch-normalized pointer,
/// folded into the bitmasks and mouse fields the contract declares.
pub fn keys_of(t: &Terminal, touch: &mut Touch, play: bool, areas: &Areas) -> Keys {
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
    let (mouse, mouse_down) = touch.update(play, areas, &t.pointer);
    Keys {
        down: fold(&|c| t.input.is_down(c)),
        pressed: fold(&|c| t.input.just_pressed(c)),
        released: fold(&|c| t.input.just_released(c)),
        mouse,
        mouse_down,
    }
}

/// A synthetic one-panel `Areas`: the whole frame is the viewport, with the
/// same 1-cell border exclusion a real IDE layout applies, so `normalize`
/// sees the identical shape it would from a live `Terminal`.
fn synthetic_viewport(size: (u32, u32)) -> Areas {
    let (w, h) = size;
    let rect = Rect {
        x: 0,
        y: 0,
        width: (w as u16).saturating_add(2),
        height: (h as u16).saturating_add(2),
    };
    Areas { rects: [(Panel::Viewport, rect); 5] }
}

/// `shinra shot` has no terminal, so there is no [`Pointer`] for a real tap to
/// feed. This replays a fixed, deterministic sequence of pointer events read
/// once from a file — one per frame, same polling shape as
/// [`crate::pad::ScriptedPad`] — through the identical `Pointer` ->
/// [`Touch::update`] -> [`normalize`] path a live terminal drives, so a
/// headless screenshot proves the whole chain.
pub struct ScriptedTouch {
    events: Vec<Option<(MouseEventKind, u16, u16)>>,
    pointer: Pointer,
    touch: Touch,
    areas: Areas,
}

impl ScriptedTouch {
    /// Reads the line format [`parse_touch`] describes. `size` is the
    /// headless framebuffer size, which stands in for the viewport a real
    /// IDE layout would have measured.
    pub fn open(path: &Path, size: (u32, u32)) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(ScriptedTouch {
            events: parse_touch(&text)?,
            pointer: Pointer::new(),
            touch: Touch::new(),
            areas: synthetic_viewport(size),
        })
    }

    /// Feeds this frame's scripted event (if any) into the pointer, then runs
    /// the same `Touch::update` a live terminal does, always in Play mode —
    /// headless has no IDE to steal clicks for. Keyboard bits stay zero: a
    /// scripted touch file drives only the pointer. Once the script runs out,
    /// the last line repeats, same rule [`crate::pad::ScriptedPad`] uses.
    pub fn poll(&mut self, frame: u32) -> Keys {
        let ev = self
            .events
            .get(frame as usize)
            .or(self.events.last())
            .copied()
            .flatten();
        if let Some((kind, col, row)) = ev {
            self.pointer.feed(kind, col, row);
        }
        let (mouse, mouse_down) = self.touch.update(true, &self.areas, &self.pointer);
        Keys { down: 0, pressed: 0, released: 0, mouse, mouse_down }
    }
}

/// One line per frame: `down COL,ROW`, `drag COL,ROW`, `up COL,ROW`, or
/// `move COL,ROW` (a plain reposition with no button change), or `-` for no
/// event this frame (the pointer holds its latched state, same as a live
/// terminal would between reports). Blank lines and lines starting with `#`
/// are skipped.
///
/// ```text
/// -
/// down 5,5
/// drag 6,6
/// up 6,6
/// ```
fn parse_touch(text: &str) -> Result<Vec<Option<(MouseEventKind, u16, u16)>>> {
    let mut events = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "-" {
            events.push(None);
            continue;
        }
        let (cmd, rest) = line
            .split_once(' ')
            .with_context(|| format!("bad touch line `{line}`"))?;
        let (col, row) = rest
            .trim()
            .split_once(',')
            .with_context(|| format!("bad touch position in `{line}`"))?;
        let col: u16 = col
            .trim()
            .parse()
            .with_context(|| format!("bad column in `{line}`"))?;
        let row: u16 = row
            .trim()
            .parse()
            .with_context(|| format!("bad row in `{line}`"))?;
        let kind = match cmd {
            "down" => MouseEventKind::Down(MouseButton::Left),
            "drag" => MouseEventKind::Drag(MouseButton::Left),
            "up" => MouseEventKind::Up(MouseButton::Left),
            "move" => MouseEventKind::Moved,
            _ => bail!("unknown touch command `{cmd}`"),
        };
        events.push(Some((kind, col, row)));
    }
    Ok(events)
}

/// Proves the whole chain end to end: a contract that declares no `pad`, a
/// game module that reads `arena::INPUT.mouse_down` (the engine's own
/// control var, always injected — see `crates/se-contract/src/engine.wit`)
/// and writes it to an observable `look` var, driven by a real
/// [`ScriptedTouch`] through the same `set_input`/`Vm::frame` sequence
/// `shot::run` and `run::run` use. Lives here rather than in `se-vm`'s or
/// `se-cli`'s `tests/`, for the same reason `pad.rs`'s `vm_integration`
/// module does: `ScriptedTouch` is private to this binary crate.
#[cfg(test)]
mod vm_integration;

#[cfg(test)]
mod tests;
