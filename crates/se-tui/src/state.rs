//! What the IDE draws. Plain data, filled by the host each frame.
//!
//! Deliberately free of engine types: the panels render a snapshot, so the
//! IDE cannot accidentally become a second place where world state lives.

/// What the IDE is doing. It opens in `Edit`: an engine editor shows you a
/// scene to work on, it does not start playing at you. `r` toggles.
///
/// In `Edit` the world exists — control's `start` has reconciled the roster —
/// but no tick runs, so nothing moves and the Inspector shows a still frame
/// you can actually read. In `Play` the clock runs and the keyboard belongs to
/// the game.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Edit,
    Play,
}

impl Mode {
    pub fn is_play(self) -> bool {
        matches!(self, Mode::Play)
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Edit => "■ EDIT",
            Mode::Play => "▶ PLAY",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Panel {
    Hierarchy,
    /// Where focus starts: the picture is the point.
    #[default]
    Viewport,
    Inspector,
    Project,
    Terminal,
}

pub const PANEL_ORDER: [Panel; 5] = [
    Panel::Hierarchy,
    Panel::Viewport,
    Panel::Inspector,
    Panel::Project,
    Panel::Terminal,
];

impl Panel {
    pub fn next(self) -> Panel {
        let i = PANEL_ORDER.iter().position(|&p| p == self).unwrap_or(0);
        PANEL_ORDER[(i + 1) % PANEL_ORDER.len()]
    }
}

/// One entity, as the Hierarchy lists it.
pub struct Node {
    pub label: String,
    /// Component names it carries — what expanding the node reveals.
    pub components: Vec<String>,
}

pub struct Ide {
    /// The deck. `n` moves through it; this is GameTok's swipe, on a keyboard.
    pub games: Vec<String>,
    pub current: usize,

    pub adapter: String,
    pub fps: f32,
    pub frame: u64,

    pub hierarchy: Vec<Node>,
    pub selected: usize,
    /// Which entities are showing their components. Keyed by label rather
    /// than by row, because the roster is rebuilt from the world every frame
    /// and a row index means a different entity the moment one despawns.
    pub expanded: std::collections::HashSet<String>,
    /// `(field, value)` rows for the selected entity.
    pub inspector: Vec<(String, String)>,
    /// Which of those rows hold a value that can be typed into. Component
    /// headers are false, and so is anything the value column truncates.
    /// Filled by the host: the IDE draws rows, the host knows what one means.
    pub inspector_editable: Vec<bool>,
    pub inspector_sel: usize,
    /// The keystrokes so far, while a field is being edited. The IDE holds the
    /// text; the host holds the meaning and does the writing.
    pub editing: Option<String>,
    /// The bundle's module tree, as the layout rule lays it out.
    pub project: Vec<String>,
    /// Which project lines are modules that can be swapped in. Filled by the host.
    pub project_pick: Vec<bool>,
    pub project_sel: usize,
    pub log: crate::log::Log,

    pub focused: Panel,
    pub mode: Mode,
    /// How the viewport spends a character cell. `m` cycles it — but only in
    /// the editor: once a game is running, `m` is whatever that game says it
    /// is, and game1 says it changes the model.
    pub art: crate::textart::TextArtMode,
}

impl Default for Ide {
    fn default() -> Ide {
        Ide {
            games: Vec::new(),
            current: 0,
            adapter: String::new(),
            fps: 0.0,
            frame: 0,
            hierarchy: Vec::new(),
            selected: 0,
            expanded: std::collections::HashSet::new(),
            inspector: Vec::new(),
            inspector_editable: Vec::new(),
            inspector_sel: 0,
            editing: None,
            project: Vec::new(),
            project_pick: Vec::new(),
            project_sel: 0,
            log: crate::log::Log::default(),
            focused: Panel::default(),
            mode: Mode::default(),
            art: crate::textart::TextArtMode::Mixed,
        }
    }
}

impl Ide {
    /// Cycle the viewport glyph set, the way `m` did in the original.
    pub fn cycle_art(&mut self) -> &'static str {
        use crate::textart::TextArtMode::*;
        self.art = match self.art {
            Mixed => Quadrant,
            Quadrant => Braille,
            Braille => Mixed,
        };
        match self.art {
            Mixed => "mixed",
            Quadrant => "quadrant",
            Braille => "braille",
        }
    }

    pub fn game(&self) -> &str {
        self.games.get(self.current).map(|s| s.as_str()).unwrap_or("-")
    }

    /// `game 2/3: game2`, as the original menu bar put it.
    pub fn game_label(&self) -> String {
        format!(
            "game {}/{}: {}",
            self.current + 1,
            self.games.len().max(1),
            self.game()
        )
    }

    /// Show or hide the selected entity's components.
    ///
    /// Selection stays on entities — the component rows are a disclosure, not
    /// somewhere to navigate to — so `selected` keeps meaning what the rest of
    /// the host already assumes it means.
    pub fn set_expanded(&mut self, open: bool) -> bool {
        let Some(n) = self.hierarchy.get(self.selected) else { return false };
        if n.components.is_empty() {
            return false;
        }
        let label = n.label.clone();
        if open {
            self.expanded.insert(label)
        } else {
            self.expanded.remove(&label)
        }
    }

    pub fn is_expanded(&self, n: &Node) -> bool {
        self.expanded.contains(&n.label)
    }

    /// Step to the next inspector row that can actually be edited, skipping
    /// component headers on the way.
    pub fn move_inspector(&mut self, delta: i32) {
        let n = self.inspector_editable.len();
        if n == 0 {
            return;
        }
        let mut i = self.inspector_sel.min(n - 1);
        for _ in 0..n {
            i = ((i as i32 + delta).rem_euclid(n as i32)) as usize;
            if self.inspector_editable[i] {
                self.inspector_sel = i;
                return;
            }
        }
    }

    /// Start typing into the selected row, seeded with what it already says.
    pub fn begin_edit(&mut self) -> bool {
        if !self.inspector_editable.get(self.inspector_sel).copied().unwrap_or(false) {
            return false;
        }
        let cur = self.inspector.get(self.inspector_sel).map(|(_, v)| v.clone());
        self.editing = Some(cur.unwrap_or_default());
        true
    }

    pub fn edit_push(&mut self, c: char) {
        if let Some(b) = self.editing.as_mut() {
            if b.len() < 64 {
                b.push(c);
            }
        }
    }

    pub fn edit_backspace(&mut self) {
        if let Some(b) = self.editing.as_mut() {
            b.pop();
        }
    }

    /// Hand the typed text to the host and leave edit mode.
    pub fn take_edit(&mut self) -> Option<(usize, String)> {
        self.editing.take().map(|t| (self.inspector_sel, t))
    }

    /// Keep the selection valid across a frame that changed the roster.
    pub fn clamp_inspector(&mut self) {
        let n = self.inspector_editable.len();
        if n == 0 {
            self.inspector_sel = 0;
        } else if self.inspector_sel >= n || !self.inspector_editable[self.inspector_sel] {
            self.inspector_sel = self
                .inspector_editable
                .iter()
                .position(|&e| e)
                .unwrap_or(0);
        }
    }

    /// Step to the next swappable project line, wrapping past the ends.
    pub fn move_project(&mut self, delta: i32) {
        let n = self.project_pick.len();
        if n == 0 {
            return;
        }
        let mut i = self.project_sel.min(n - 1);
        for _ in 0..n {
            i = ((i as i32 + delta).rem_euclid(n as i32)) as usize;
            if self.project_pick[i] {
                self.project_sel = i;
                return;
            }
        }
    }

    /// Keep the project cursor on a swappable line, or 0 when there is none.
    pub fn clamp_project(&mut self) {
        let n = self.project_pick.len();
        if n == 0 {
            self.project_sel = 0;
        } else if self.project_sel >= n || !self.project_pick[self.project_sel] {
            self.project_sel = self.project_pick.iter().position(|&e| e).unwrap_or(0);
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.hierarchy.is_empty() {
            return;
        }
        let n = self.hierarchy.len() as i32;
        self.selected = (((self.selected as i32 + delta) % n + n) % n) as usize;
    }

    pub fn note(&mut self, s: impl Into<String>) {
        self.log.push(&s.into(), crate::log::Level::Info);
    }

    /// Like `note`, drawn in red. A multi-line message is split and capped.
    pub fn error(&mut self, s: impl Into<String>) {
        self.log.push(&s.into(), crate::log::Level::Error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ide() -> Ide {
        Ide {
            inspector: vec![("Transform".into(), String::new()), ("x".into(), "1.5".into())],
            inspector_editable: vec![false, true],
            ..Default::default()
        }
    }

    #[test]
    fn project_moves_over_pickable_and_wraps() {
        let mut i = Ide { project_pick: vec![false, true, false, true], project_sel: 1, ..Default::default() };
        i.move_project(1);
        assert_eq!(i.project_sel, 3);
        i.move_project(1);
        assert_eq!(i.project_sel, 1);
        i.move_project(-1);
        assert_eq!(i.project_sel, 3);
    }

    #[test]
    fn project_clamp() {
        let mut i = Ide { project_pick: vec![false, false], project_sel: 1, ..Default::default() };
        i.clamp_project();
        assert_eq!(i.project_sel, 0);
        i.project_pick = vec![false, false, true];
        i.clamp_project();
        assert_eq!(i.project_sel, 2);
        i.project_pick.clear();
        i.clamp_project();
        assert_eq!(i.project_sel, 0);
    }

    #[test]
    fn begin_on_header_is_refused() {
        let mut i = ide();
        i.inspector_sel = 0;
        assert!(!i.begin_edit());
        assert!(i.editing.is_none());
    }

    #[test]
    fn begin_seeds_current_text_and_take_returns_it() {
        let mut i = ide();
        i.inspector_sel = 1;
        assert!(i.begin_edit());
        assert_eq!(i.editing.as_deref(), Some("1.5"));
        for c in ['2', '3', '4'] {
            i.edit_push(c);
        }
        i.edit_backspace();
        assert_eq!(i.editing.as_deref(), Some("1.523"));
        assert_eq!(i.take_edit(), Some((1, "1.523".to_string())));
        assert!(i.editing.is_none());
    }

    #[test]
    fn buffer_caps_at_64() {
        let mut i = ide();
        i.inspector_sel = 1;
        i.begin_edit();
        for _ in 0..65 {
            i.edit_push('9');
        }
        assert_eq!(i.editing.as_ref().map(|b| b.len()), Some(64));
    }
}
