//! Raw mode, and turning keystrokes into `Input`.
//!
//! A terminal reports key *presses*, never releases. So held state is
//! synthesised: a key counts as down for a short grace period after its last
//! press, which auto-repeat keeps refreshing. This is a real limitation of the
//! surface, not of the engine — a windowed presenter fills `Input` exactly.

use anyhow::Result;
use crate::slot::{slot_key, SlotKey};
use crate::state::Panel;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use crossterm::terminal;
use crate::keys::{key, Input, KEY_COUNT};
use std::time::{Duration, Instant};

/// How long after its last press a key still counts as held. Comfortably
/// longer than the ~30ms of a key-repeat interval, short enough that a real
/// release is noticed within a frame or two.
const HELD_FOR: Duration = Duration::from_millis(140);

/// What the operator asked of a field being edited.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edit {
    Begin,
    Commit,
    Cancel,
}

/// IDE-level intents, handled by the host rather than passed to the game.
#[derive(Default, Clone, Copy)]
pub struct Intent {
    /// `n` — the next bundle in the deck. GameTok's swipe, on a keyboard.
    pub next_game: bool,
    pub prev_game: bool,
    pub cycle_panel: bool,
    pub select: i32,
    /// `r` — start the game, or stop it and go back to the scene.
    pub toggle_run: bool,
    /// Esc while playing means "leave the run", not "quit the editor".
    pub leave_run: bool,
    /// `m` — cycle the viewport glyph set. Editor only: while a game runs,
    /// `m` is whatever that game decided it means.
    pub cycle_art: bool,
    /// Right/Left on the Hierarchy: disclose the selected entity's components
    /// or fold them away. `Some(true)` opens, `Some(false)` closes.
    pub expand: Option<bool>,
    /// Up/Down while the Inspector has focus — its own selection, not the
    /// Hierarchy's.
    pub inspect_select: i32,
    /// Up/Down while the Project panel has focus.
    pub project_select: i32,
    /// Enter while the Project panel has focus: swap in the selected module.
    pub swap: bool,
    pub edit: Option<Edit>,
    pub type_char: Option<char>,
    pub type_back: bool,
    /// A panel to focus, from ctrl+h/v/i/f/t or a mouse click.
    pub focus: Option<crate::state::Panel>,
    /// Where the mouse was pressed, for the host to resolve against panels.
    pub click: Option<(u16, u16)>,
    pub reload: bool,
    /// F5 / F9 — quick save and quick load, in both modes.
    pub save: bool,
    pub load: bool,
}

/// Where stderr goes while the IDE owns the screen.
///
/// The graphics stack writes to stderr whether or not anyone asked — Mesa
/// alone emits several lines picking a software adapter — and stderr is the
/// same terminal the panels are drawn on, so those lines land *inside* the
/// layout and corrupt it. Anything that takes over the screen has to take
/// over stderr with it. The text is not discarded: it goes to a file the run
/// prints on the way out, because a driver complaint is often the reason a
/// frame looks wrong.
struct StderrTo {
    saved: i32,
    pub path: std::path::PathBuf,
}

impl StderrTo {
    fn file(path: std::path::PathBuf) -> Option<StderrTo> {
        use std::os::unix::io::AsRawFd;
        let f = std::fs::File::create(&path).ok()?;
        // SAFETY: plain fd arithmetic on our own process's stderr.
        unsafe {
            let saved = libc::dup(libc::STDERR_FILENO);
            if saved < 0 {
                return None;
            }
            if libc::dup2(f.as_raw_fd(), libc::STDERR_FILENO) < 0 {
                libc::close(saved);
                return None;
            }
            Some(StderrTo { saved, path })
        }
    }
}

impl Drop for StderrTo {
    fn drop(&mut self) {
        // SAFETY: `saved` is the descriptor `dup` handed us and is still open.
        unsafe {
            libc::dup2(self.saved, libc::STDERR_FILENO);
            libc::close(self.saved);
        }
    }
}

pub struct Terminal {
    raw: bool,
    /// Restored when this is dropped, so a panic still leaves a usable shell.
    stderr: Option<StderrTo>,
    last_press: Vec<Option<Instant>>,
    prev_down: [u8; KEY_COUNT],
    pub input: Input,
    pub quit: bool,
    pub swipe: i32,
    pub reload: bool,
    pub intent: Intent,
    /// Set by the host before each poll. While playing, the keyboard is the
    /// game's and the IDE keeps only the keys that leave or switch a run.
    pub play: bool,
    /// Set by the host while a field is being typed into. The keyboard is a
    /// text box then: Esc cancels the edit instead of quitting, and no IDE
    /// shortcut fires — an `n` in a number is a keystroke, not a game change.
    pub typing: bool,
    /// Whether the Inspector has focus, so Up/Down and Enter can mean its
    /// selection rather than the Hierarchy's.
    pub on_inspector: bool,
    /// Whether the Project panel has focus.
    pub on_project: bool,
}

impl Terminal {
    pub fn enter() -> Result<Terminal> {
        let log = std::env::temp_dir().join("shinra-stderr.log");
        let stderr = StderrTo::file(log);
        terminal::enable_raw_mode()?;
        let mut out = std::io::stdout();
        use std::io::Write;
        // Alternate screen, hidden cursor. Both restored on drop, including
        // on the panic path, or the user is left with an unusable shell.
        write!(out, "\x1b[?1049h\x1b[?25l\x1b[2J")?;
        // Clicking a panel focuses it, as the original did.
        crossterm::execute!(out, EnableMouseCapture)?;
        out.flush()?;
        Ok(Terminal {
            raw: true,
            stderr,
            last_press: vec![None; KEY_COUNT],
            prev_down: [0; KEY_COUNT],
            input: Input::zeroed(),
            quit: false,
            swipe: 0,
            reload: false,
            intent: Intent::default(),
            typing: false,
            on_inspector: false,
            on_project: false,
            play: false,
        })
    }

    /// Where the driver's chatter went, for the caller to mention on exit.
    pub fn stderr_log(&self) -> Option<&std::path::Path> {
        self.stderr.as_ref().map(|s| s.path.as_path())
    }

    pub fn size(&self) -> Result<(u16, u16)> {
        Ok(terminal::size()?)
    }

    /// Drain pending events and rebuild `input` for this frame.
    pub fn poll(&mut self) -> Result<()> {
        self.swipe = 0;
        self.reload = false;
        self.intent = Intent::default();
        self.input.pressed = [0; KEY_COUNT];
        self.input.released = [0; KEY_COUNT];

        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
                Event::Mouse(m) => {
                    if matches!(m.kind, MouseEventKind::Down(_)) {
                        self.intent.click = Some((m.column, m.row));
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        let now = Instant::now();
        for i in 0..KEY_COUNT {
            let down = self.last_press[i].is_some_and(|t| now.duration_since(t) < HELD_FOR);
            self.input.down[i] = down as u8;
            if !down && self.prev_down[i] != 0 {
                self.input.released[i] = 1;
            }
            self.prev_down[i] = down as u8;
        }
        self.input.swipe = self.swipe as i8;
        Ok(())
    }

    fn on_key(&mut self, k: KeyEvent) {
        // Host-level bindings first: these never reach the game.
        if k.modifiers.contains(KeyModifiers::CONTROL) {
            // The original's focus bindings, plus the two the runner needs.
            self.intent.focus = match k.code {
                KeyCode::Char('h') => Some(Panel::Hierarchy),
                KeyCode::Char('v') => Some(Panel::Viewport),
                KeyCode::Char('i') => Some(Panel::Inspector),
                KeyCode::Char('f') => Some(Panel::Project),
                KeyCode::Char('t') => Some(Panel::Terminal),
                _ => None,
            };
            match k.code {
                KeyCode::Char('c') => self.quit = true,
                KeyCode::Char('r') => self.reload = true,
                _ => {}
            }
            return;
        }
        // A field being edited swallows the keyboard. This sits ahead of the
        // Esc branch on purpose: while typing, Esc means "never mind this
        // value", and quitting the IDE instead would be a nasty surprise.
        if self.typing {
            match k.code {
                KeyCode::Esc => self.intent.edit = Some(Edit::Cancel),
                KeyCode::Enter => self.intent.edit = Some(Edit::Commit),
                KeyCode::Backspace => self.intent.type_back = true,
                KeyCode::Char(c) => self.intent.type_char = Some(c),
                _ => {}
            }
            return;
        }

        match slot_key(k.code) {
            Some(SlotKey::Save) => self.intent.save = true,
            Some(SlotKey::Load) => self.intent.load = true,
            None => {}
        }
        if slot_key(k.code).is_some() {
            return;
        }

        // Esc leaves a run; only in the editor does it quit.
        if k.code == KeyCode::Esc {
            if self.play {
                self.intent.leave_run = true;
            } else {
                self.quit = true;
            }
            return;
        }

        // `r` and `n` are the host's in both modes — the original bound them
        // that way, and a game that could shadow them could trap the player
        // inside itself.
        match k.code {
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.intent.toggle_run = true;
                return;
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.intent.next_game = true;
                return;
            }
            _ => {}
        }

        // `q` quits, but only from the editor: while a game is running `q` is
        // the game's, and quitting the whole IDE on a stray keypress mid-play
        // is not what anyone meant.
        if !self.play && matches!(k.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
            self.quit = true;
            return;
        }

        // Everything else while playing is gameplay — including `m`, which is
        // game1's model swap and none of the IDE's business.
        if self.play {
            if let Some(code) = map(k.code) {
                self.last_press[code as usize] = Some(Instant::now());
                self.input.pressed[code as usize] = 1;
            }
            return;
        }

        // IDE keys. These never reach the game: the deck and the panels belong
        // to the host, and a game that could see `n` could steal it.
        match k.code {
            KeyCode::Char('m') | KeyCode::Char('M') => {
                self.intent.cycle_art = true;
                return;
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                self.intent.prev_game = true;
                return;
            }
            KeyCode::Tab => {
                self.intent.cycle_panel = true;
                return;
            }
            KeyCode::Up if self.on_inspector => {
                self.intent.inspect_select = -1;
                return;
            }
            KeyCode::Down if self.on_inspector => {
                self.intent.inspect_select = 1;
                return;
            }
            KeyCode::Enter if self.on_inspector => {
                self.intent.edit = Some(Edit::Begin);
                return;
            }
            KeyCode::Up if self.on_project => {
                self.intent.project_select = -1;
                return;
            }
            KeyCode::Down if self.on_project => {
                self.intent.project_select = 1;
                return;
            }
            KeyCode::Enter if self.on_project => {
                self.intent.swap = true;
                return;
            }
            KeyCode::Up => self.intent.select = -1,
            KeyCode::Down => self.intent.select = 1,
            // Unshifted, because shift+arrow is the swipe below.
            KeyCode::Right | KeyCode::Enter if !k.modifiers.contains(KeyModifiers::SHIFT) => {
                self.intent.expand = Some(true);
                return;
            }
            KeyCode::Left if !k.modifiers.contains(KeyModifiers::SHIFT) => {
                self.intent.expand = Some(false);
                return;
            }
            _ => {}
        }

        // GameTok: a horizontal swipe is how you leave one game for the next.
        if k.modifiers.contains(KeyModifiers::SHIFT) {
            match k.code {
                KeyCode::Right => {
                    self.swipe = 1;
                    return;
                }
                KeyCode::Left => {
                    self.swipe = -1;
                    return;
                }
                _ => {}
            }
        }

        if let Some(code) = map(k.code) {
            self.last_press[code as usize] = Some(Instant::now());
            self.input.pressed[code as usize] = 1;
        }
    }
}

fn map(c: KeyCode) -> Option<u8> {
    Some(match c {
        KeyCode::Char(ch) if ch.is_ascii() => (ch as u8).to_ascii_uppercase(),
        KeyCode::Left => key::LEFT,
        KeyCode::Right => key::RIGHT,
        KeyCode::Up => key::UP,
        KeyCode::Down => key::DOWN,
        KeyCode::Enter => key::ENTER,
        KeyCode::Tab => key::TAB,
        KeyCode::Backspace => key::BACKSPACE,
        _ => return None,
    })
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.raw {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = crossterm::execute!(out, DisableMouseCapture);
            let _ = write!(out, "\x1b[0m\x1b[?25h\x1b[?1049l");
            let _ = out.flush();
            let _ = terminal::disable_raw_mode();
        }
    }
}
