//! A pure pointer state machine: raw terminal-cell position plus
//! held/not-held, driven by fed `MouseEventKind` values. No tty or
//! `Terminal` involved, so it is unit-testable without a real terminal.

use crossterm::event::MouseEventKind;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Pointer {
    pos: (u16, u16),
    held: bool,
}

impl Pointer {
    pub fn new() -> Pointer {
        Pointer::default()
    }

    /// Current raw terminal-cell position (column, row).
    pub fn pos(&self) -> (u16, u16) {
        self.pos
    }

    /// Whether a button/touch is currently held.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Feed one mouse event at its reported cell. `Down`/`Drag` set held,
    /// `Up` clears it, `Moved` only updates position.
    pub fn feed(&mut self, kind: MouseEventKind, col: u16, row: u16) {
        self.pos = (col, row);
        match kind {
            MouseEventKind::Down(_) | MouseEventKind::Drag(_) => self.held = true,
            MouseEventKind::Up(_) => self.held = false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::MouseButton;

    #[test]
    fn down_sets_held_and_position() {
        let mut p = Pointer::new();
        p.feed(MouseEventKind::Down(MouseButton::Left), 5, 7);
        assert_eq!(p.pos(), (5, 7));
        assert!(p.held());
    }

    #[test]
    fn drag_while_held_updates_position_and_stays_held() {
        let mut p = Pointer::new();
        p.feed(MouseEventKind::Down(MouseButton::Left), 5, 7);
        p.feed(MouseEventKind::Drag(MouseButton::Left), 9, 2);
        assert_eq!(p.pos(), (9, 2));
        assert!(p.held());
    }

    #[test]
    fn up_clears_held_and_keeps_last_position() {
        let mut p = Pointer::new();
        p.feed(MouseEventKind::Down(MouseButton::Left), 5, 7);
        p.feed(MouseEventKind::Drag(MouseButton::Left), 9, 2);
        p.feed(MouseEventKind::Up(MouseButton::Left), 9, 2);
        assert_eq!(p.pos(), (9, 2));
        assert!(!p.held());
    }

    #[test]
    fn moved_while_not_held_updates_position_only() {
        let mut p = Pointer::new();
        p.feed(MouseEventKind::Moved, 3, 4);
        assert_eq!(p.pos(), (3, 4));
        assert!(!p.held());
    }
}
