//! The two host keys that save and load a game. F-keys, so no letter the game
//! might want is taken.

use crossterm::event::KeyCode;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotKey {
    Save,
    Load,
}

/// F5 saves, F9 loads; every other key is not a slot key.
pub fn slot_key(code: KeyCode) -> Option<SlotKey> {
    match code {
        KeyCode::F(5) => Some(SlotKey::Save),
        KeyCode::F(9) => Some(SlotKey::Load),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f5_saves_f9_loads() {
        assert_eq!(slot_key(KeyCode::F(5)), Some(SlotKey::Save));
        assert_eq!(slot_key(KeyCode::F(9)), Some(SlotKey::Load));
    }

    #[test]
    fn other_keys_are_not_slots() {
        assert_eq!(slot_key(KeyCode::Char('s')), None);
        assert_eq!(slot_key(KeyCode::Char('l')), None);
        assert_eq!(slot_key(KeyCode::F(1)), None);
    }
}
