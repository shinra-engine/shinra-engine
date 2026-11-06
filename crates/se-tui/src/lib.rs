//! Showing a frame, and the pipeline that made it.
//!
//! The design says every render pipeline gets a TUI. That is not a debug view
//! bolted on the side — the terminal is a presenter like any other, fed the
//! same presented buffer, which is why the IDE and the game are the same
//! program with the panel turned on.

pub mod ide;
mod keys;
mod log;
pub mod present;
pub mod slot;
pub mod state;
pub mod textart;
pub mod term;
pub mod touch;

pub use ide::{draw, Areas};
pub use log::{Level, LogLine};
pub use keys::{key, Input, KEY_COUNT};
pub use present::Pixels;
pub use slot::SlotKey;
pub use state::{Ide, Mode, Node, Panel, PANEL_ORDER};
pub use textart::{TextArtMode, TextCell};
pub use term::{Edit, Intent, Terminal};
