//! Turns the terminal's raw pointer into the viewport-local, Play-mode-gated
//! form `Keys.mouse` / `Keys.mouse_down` need, and assembles the rest of the
//! frame's `Keys` alongside it. No contract or se-vm change: the existing
//! 24-byte `input` record is used exactly as declared.

use se_tui::touch::Pointer;
use se_tui::{Areas, Terminal};
use se_vm::input::Keys;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::layout::Rect;
    use se_tui::Panel;

    const ORIGIN: (u16, u16) = (1, 1);
    const INNER: (u16, u16) = (11, 11);

    /// A viewport-only `Areas`, origin `(1, 1)` and inner size `(11, 11)` —
    /// a 13x13 outer rect at (0, 0), border excluded same as `viewport_inner`.
    fn viewport() -> Areas {
        let rect = Rect {
            x: 0,
            y: 0,
            width: 13,
            height: 13,
        };
        Areas {
            rects: [(Panel::Viewport, rect); 5],
        }
    }

    #[test]
    fn center_cell_maps_near_half() {
        let (x, y) = normalize(ORIGIN, INNER, 6, 6).unwrap();
        assert!((x - 0.5).abs() < 1e-6, "x={x}");
        assert!((y - 0.5).abs() < 1e-6, "y={y}");
    }

    #[test]
    fn outside_viewport_reports_not_down_with_position_unchanged() {
        let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
        p.feed(MouseEventKind::Moved, 6, 6);
        let (first, _) = t.update(true, &v, &p);
        p.feed(MouseEventKind::Moved, 50, 50);
        let (second, down) = t.update(true, &v, &p);
        assert_eq!(first, second);
        assert_eq!(down, 0);
    }

    #[test]
    fn press_starting_outside_and_dragging_inside_stays_not_down() {
        let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
        p.feed(MouseEventKind::Down(MouseButton::Left), 50, 50);
        let (_, down) = t.update(true, &v, &p);
        assert_eq!(down, 0);
        p.feed(MouseEventKind::Drag(MouseButton::Left), 6, 6);
        let (_, down) = t.update(true, &v, &p);
        assert_eq!(down, 0, "started outside — stays up even once inside");
    }

    #[test]
    fn press_starting_inside_stays_down_while_dragging_outside() {
        let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
        p.feed(MouseEventKind::Down(MouseButton::Left), 6, 6);
        let (_, down) = t.update(true, &v, &p);
        assert_eq!(down, 1);
        p.feed(MouseEventKind::Drag(MouseButton::Left), 50, 50);
        let (pos, down) = t.update(true, &v, &p);
        assert_eq!(down, 1, "started inside — stays down even once outside");
        let (cx, cy) = normalize(ORIGIN, INNER, 6, 6).unwrap();
        assert_eq!(
            pos,
            [cx, cy],
            "position holds at the last cell inside the viewport"
        );
    }

    #[test]
    fn edit_mode_never_reports_down() {
        let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
        p.feed(MouseEventKind::Down(MouseButton::Left), 6, 6);
        let (_, down) = t.update(false, &v, &p);
        assert_eq!(down, 0);
    }

    #[test]
    fn release_requires_a_fresh_press_to_go_down_again() {
        let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
        p.feed(MouseEventKind::Down(MouseButton::Left), 6, 6);
        t.update(true, &v, &p);
        p.feed(MouseEventKind::Up(MouseButton::Left), 6, 6);
        let (_, down) = t.update(true, &v, &p);
        assert_eq!(down, 0);
    }
}
