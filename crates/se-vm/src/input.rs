//! The keyboard, as a control var.
//!
//! Input used to be a *resource*: a 780-byte `Input` the host handed a stage
//! by pointer, with one byte per key for each of down, pressed and released.
//! There is nowhere to put that now, and nowhere it belongs — a game's whole
//! read surface is the arena, and a record in the contract carries no arrays.
//!
//! So the keyboard is three `u32` bitmasks. Thirty-two keys is not a
//! limitation a game notices: it is the letters, the arrows, space and enter,
//! which is every key a game has ever bound in this engine. What it buys is
//! that input is declared in `contract.wit` like everything else, written by
//! the host before the tick, and read by `game.wasm` at an address the
//! contract fixed — with no host call and no import.
//!
//! ```text
//!   bit  0..25   a..z
//!   bit     26   space
//!   bit     27   enter
//!   bit     28   left      bit 29  right
//!   bit     30   up        bit 31  down
//! ```

/// Bit for a printable letter. Anything else is [`NONE`].
pub const fn letter(c: u8) -> u32 {
    if c.is_ascii_lowercase() {
        1 << (c - b'a')
    } else if c.is_ascii_uppercase() {
        1 << (c - b'A')
    } else {
        NONE
    }
}

pub const NONE: u32 = 0;
pub const SPACE: u32 = 1 << 26;
pub const ENTER: u32 = 1 << 27;
pub const LEFT: u32 = 1 << 28;
pub const RIGHT: u32 = 1 << 29;
pub const UP: u32 = 1 << 30;
pub const DOWN: u32 = 1 << 31;

/// What the host writes into the `input` control var each frame.
///
/// Laid out to match `interface control`'s `input` record: three masks, then
/// the mouse. The host checks the contract's offsets before it writes, so a
/// game that spells the record differently is refused rather than fed
/// nonsense at the wrong offsets.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Keys {
    /// Set while held.
    pub down: u32,
    /// Set only on the frame the key went down.
    pub pressed: u32,
    /// Set only on the frame it came up.
    pub released: u32,
    pub mouse: [f32; 2],
    pub mouse_down: u32,
}

impl Keys {
    /// The 24 bytes the record occupies, in its own order.
    pub fn to_bytes(self) -> [u8; 24] {
        let mut b = [0u8; 24];
        b[0..4].copy_from_slice(&self.down.to_le_bytes());
        b[4..8].copy_from_slice(&self.pressed.to_le_bytes());
        b[8..12].copy_from_slice(&self.released.to_le_bytes());
        b[12..16].copy_from_slice(&self.mouse[0].to_le_bytes());
        b[16..20].copy_from_slice(&self.mouse[1].to_le_bytes());
        b[20..24].copy_from_slice(&self.mouse_down.to_le_bytes());
        b
    }

    /// Fold a frame's worth of key events into the three masks. `held` is what
    /// is down now; `was` is what was down last frame.
    pub fn from_held(held: u32, was: u32, mouse: [f32; 2], mouse_down: bool) -> Keys {
        Keys {
            down: held,
            pressed: held & !was,
            released: was & !held,
            mouse,
            mouse_down: mouse_down as u32,
        }
    }

    pub fn is_down(self, bit: u32) -> bool {
        self.down & bit != 0
    }
    pub fn just_pressed(self, bit: u32) -> bool {
        self.pressed & bit != 0
    }
}

/// The record's field names and offsets, as this engine writes them. Checked
/// against the contract at load, like every other handle.
pub const FIELDS: [(&str, u32); 6] = [
    ("down", 0),
    ("pressed", 4),
    ("released", 8),
    ("mouse_x", 12),
    ("mouse_y", 16),
    ("mouse_down", 20),
];
pub const BYTES: u32 = 24;

/// Gamepad button bits, for `Pad::down`/`pressed`/`released`.
///
/// ```text
///   bit  0  south      bit  1  east       bit  2  west       bit  3  north
///   bit  4  lb          bit  5  rb         bit  6  back       bit  7  start
///   bit  8  left-stick  bit  9  right-stick
///   bit 10  dpad-up     bit 11  dpad-down  bit 12  dpad-left  bit 13  dpad-right
/// ```
pub const PAD_SOUTH: u32 = 1 << 0;
pub const PAD_EAST: u32 = 1 << 1;
pub const PAD_WEST: u32 = 1 << 2;
pub const PAD_NORTH: u32 = 1 << 3;
pub const PAD_LB: u32 = 1 << 4;
pub const PAD_RB: u32 = 1 << 5;
pub const PAD_BACK: u32 = 1 << 6;
pub const PAD_START: u32 = 1 << 7;
pub const PAD_LSTICK: u32 = 1 << 8;
pub const PAD_RSTICK: u32 = 1 << 9;
pub const PAD_DPAD_UP: u32 = 1 << 10;
pub const PAD_DPAD_DOWN: u32 = 1 << 11;
pub const PAD_DPAD_LEFT: u32 = 1 << 12;
pub const PAD_DPAD_RIGHT: u32 = 1 << 13;

/// What the host writes into an optional `pad` control var each frame.
///
/// Laid out to match `interface control`'s `pad` record: three button masks,
/// then the two sticks and two triggers. The host checks the contract's
/// offsets before it writes, same rule as [`Keys`].
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Pad {
    /// Set while held.
    pub down: u32,
    /// Set only on the frame the button went down.
    pub pressed: u32,
    /// Set only on the frame it came up.
    pub released: u32,
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    pub lt: f32,
    pub rt: f32,
}

impl Pad {
    /// The 36 bytes the record occupies, in its own order.
    pub fn to_bytes(self) -> [u8; 36] {
        let mut b = [0u8; 36];
        b[0..4].copy_from_slice(&self.down.to_le_bytes());
        b[4..8].copy_from_slice(&self.pressed.to_le_bytes());
        b[8..12].copy_from_slice(&self.released.to_le_bytes());
        b[12..16].copy_from_slice(&self.lx.to_le_bytes());
        b[16..20].copy_from_slice(&self.ly.to_le_bytes());
        b[20..24].copy_from_slice(&self.rx.to_le_bytes());
        b[24..28].copy_from_slice(&self.ry.to_le_bytes());
        b[28..32].copy_from_slice(&self.lt.to_le_bytes());
        b[32..36].copy_from_slice(&self.rt.to_le_bytes());
        b
    }

    /// Fold a frame's worth of button state into the three masks. `held` is
    /// what is down now; `was` is what was down last frame.
    pub fn from_held(held: u32, was: u32, sticks: [f32; 4], triggers: [f32; 2]) -> Pad {
        Pad {
            down: held,
            pressed: held & !was,
            released: was & !held,
            lx: sticks[0],
            ly: sticks[1],
            rx: sticks[2],
            ry: sticks[3],
            lt: triggers[0],
            rt: triggers[1],
        }
    }

    pub fn is_down(self, bit: u32) -> bool {
        self.down & bit != 0
    }
    pub fn just_pressed(self, bit: u32) -> bool {
        self.pressed & bit != 0
    }
}

/// `Pad`'s field names and offsets, as this engine writes them. Checked
/// against the contract at load, like [`FIELDS`].
pub const PAD_FIELDS: [(&str, u32); 9] = [
    ("down", 0),
    ("pressed", 4),
    ("released", 8),
    ("lx", 12),
    ("ly", 16),
    ("rx", 20),
    ("ry", 24),
    ("lt", 28),
    ("rt", 32),
];
pub const PAD_BYTES: u32 = 36;
