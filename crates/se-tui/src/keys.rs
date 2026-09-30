//! Key table and the per-frame key state the terminal fills in.
//!
//! A terminal reports presses, never releases (see `term.rs`), so this stays a
//! TUI concern: the game only ever sees the bitmasks `se-cli` derives from it.

pub const KEY_COUNT: usize = 256;

/// Key codes. Printable ASCII maps to itself, so `b'M' as usize` indexes the
/// M key and the named constants cover the rest.
pub mod key {
    pub const ESC: u8 = 27;
    pub const SPACE: u8 = 32;
    pub const ENTER: u8 = 13;
    pub const TAB: u8 = 9;
    pub const BACKSPACE: u8 = 8;
    pub const LEFT: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const UP: u8 = 3;
    pub const DOWN: u8 = 4;
}

#[derive(Clone, Copy)]
pub struct Input {
    /// 1 while held.
    pub down: [u8; KEY_COUNT],
    /// 1 only on the frame the key went down.
    pub pressed: [u8; KEY_COUNT],
    /// 1 only on the frame the key came up.
    pub released: [u8; KEY_COUNT],
    pub mouse: [f32; 2],
    pub mouse_down: u8,
    /// GameTok: +1 swiped to the next game, -1 to the previous, 0 otherwise.
    pub swipe: i8,
    pub _pad: [u8; 2],
}

impl Input {
    pub const fn zeroed() -> Input {
        Input {
            down: [0; KEY_COUNT],
            pressed: [0; KEY_COUNT],
            released: [0; KEY_COUNT],
            mouse: [0.0, 0.0],
            mouse_down: 0,
            swipe: 0,
            _pad: [0; 2],
        }
    }

    pub fn is_down(&self, k: u8) -> bool {
        self.down[k as usize] != 0
    }
    pub fn just_pressed(&self, k: u8) -> bool {
        self.pressed[k as usize] != 0
    }
    pub fn just_released(&self, k: u8) -> bool {
        self.released[k as usize] != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_table() {
        assert_eq!(KEY_COUNT, 256);
        assert_eq!(key::ESC, 27);
        assert_eq!(key::SPACE, 32);
        assert_eq!(key::ENTER, 13);
        assert_eq!(key::TAB, 9);
        assert_eq!(key::BACKSPACE, 8);
        assert_eq!(
            [key::LEFT, key::RIGHT, key::UP, key::DOWN],
            [1, 2, 3, 4]
        );
    }

    #[test]
    fn zeroed_reports_nothing() {
        let i = Input::zeroed();
        for k in 0..=255u8 {
            assert!(!i.is_down(k) && !i.just_pressed(k) && !i.just_released(k));
        }
    }
}
