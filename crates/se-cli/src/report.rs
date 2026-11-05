//! How a failure reaches the Terminal panel.
//!
//! Pure functions over `Ide`, so they are tested without a terminal. A
//! failure the engine survived — a refused reload, a frame that rolled back —
//! is a message and nothing else: the mode stays where the user put it.

use crate::reload::Outcome;
use se_tui::{Ide, Mode};
use se_vm::memory::Slot;

/// Report one poll: what reloaded, and each unit that did not.
pub fn reload(ide: &mut Ide, out: &Outcome, ms: u128) {
    let Outcome::Changed { ok, failed } = out else { return };
    if !ok.is_empty() {
        ide.note(format!("reloaded {} in {ms} ms", ok.join(", ")));
    }
    for f in failed {
        ide.error(format!("{}: {}", f.unit, f.message));
        ide.note("kept running the previous module");
    }
}

/// Report a failed frame. With a rollback the previous modules already run
/// again, so the mode is left alone; without one the game stops in Edit.
pub fn frame_error(ide: &mut Ide, err: &anyhow::Error, rolled_back: Option<Vec<Slot>>) {
    match rolled_back {
        Some(slots) => {
            ide.error(format!("{err:#}"));
            let names: Vec<&str> = slots.iter().map(|s| s.name()).collect();
            ide.note(format!("rolled back {}: the previous module is running", names.join(", ")));
        }
        None => {
            ide.note(format!("frame failed: {err:#}"));
            ide.mode = Mode::Edit;
        }
    }
}

/// Report a GPU timeout recovered by reloading the render module that last
/// produced a picture, in the same style `frame_error` uses for a
/// rolled-back CPU trap.
pub fn render_timeout(ide: &mut Ide, err: &anyhow::Error) {
    ide.error(format!("{err:#}"));
    ide.note("rolled back render: the previous module is running".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reload::Failure;
    use se_tui::Level;

    #[test]
    fn a_failed_unit_is_red_lines_then_kept() {
        let mut ide = Ide::default();
        let before = ide.log.len();
        let failed = vec![Failure { unit: "game/game2".into(), message: "error: a\n  --> b\nc".into() }];
        reload(&mut ide, &Outcome::Changed { ok: vec![], failed }, 5);
        let new: Vec<_> = ide.log.iter().skip(before).collect();
        assert_eq!(new.len(), 4);
        assert!(new[..3].iter().all(|l| l.level == Level::Error));
        assert!(new[0].text.starts_with("game/game2: "));
        assert_eq!(new[3].level, Level::Info);
        assert_eq!(new[3].text, "kept running the previous module");
    }

    #[test]
    fn an_ok_reload_is_one_note() {
        let mut ide = Ide::default();
        let before = ide.log.len();
        reload(&mut ide, &Outcome::Changed { ok: vec!["game/game2".into()], failed: vec![] }, 7);
        let new: Vec<_> = ide.log.iter().skip(before).collect();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].level, Level::Info);
        assert!(new[0].text.contains("reloaded game/game2"));
    }

    #[test]
    fn a_rollback_keeps_play() {
        let mut ide = Ide::default();
        ide.mode = Mode::Play;
        let before = ide.log.len();
        frame_error(&mut ide, &anyhow::anyhow!("trap"), Some(vec![Slot::Game]));
        assert_eq!(ide.mode, Mode::Play);
        assert!(ide.log.iter().skip(before).any(|l| l.level == Level::Error));
    }

    #[test]
    fn no_rollback_drops_to_edit() {
        let mut ide = Ide::default();
        ide.mode = Mode::Play;
        frame_error(&mut ide, &anyhow::anyhow!("trap"), None);
        assert_eq!(ide.mode, Mode::Edit);
        assert!(ide.log.iter().any(|l| l.text.contains("frame failed")));
    }

    #[test]
    fn a_gpu_timeout_rollback_is_an_error_then_a_note() {
        let mut ide = Ide::default();
        let before = ide.log.len();
        render_timeout(&mut ide, &anyhow::anyhow!("GPU replay exceeded its budget"));
        let new: Vec<_> = ide.log.iter().skip(before).collect();
        assert_eq!(new.len(), 2);
        assert_eq!(new[0].level, Level::Error);
        assert_eq!(new[1].level, Level::Info);
        assert_eq!(new[1].text, "rolled back render: the previous module is running");
    }
}
