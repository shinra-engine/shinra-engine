//! What the operator asked of the IDE this frame, applied to the IDE state and
//! (for a committed edit) to the world.

use crate::edit;
use crate::inspect::Target;
use se_tui::term::{Edit, Intent};
use se_tui::{Areas, Ide, Mode, Panel};
use se_vm::Vm;

pub fn apply(it: Intent, ide: &mut Ide, vm: &mut Vm, row_map: &[Option<Target>], areas: &Areas) {
    if it.cycle_panel {
        ide.focused = ide.focused.next();
    }
    if let Some(p) = it.focus {
        ide.focused = p;
    }
    if let Some((c, r)) = it.click {
        if let Some(p) = areas.panel_at(c, r) {
            ide.focused = p;
        }
    }
    if it.select != 0 {
        ide.move_selection(it.select);
    }
    if let Some(o) = it.expand {
        ide.set_expanded(o);
    }
    if it.inspect_select != 0 {
        ide.move_inspector(it.inspect_select);
    }
    if it.cycle_art {
        let name = ide.cycle_art();
        ide.note(format!("viewport: {name}"));
    }
    if it.toggle_run {
        ide.mode = if ide.mode.is_play() { Mode::Edit } else { Mode::Play };
        if ide.mode.is_play() {
            ide.focused = Panel::Viewport;
        }
        ide.note(if ide.mode.is_play() { "run" } else { "stopped" });
    }
    if it.leave_run {
        ide.mode = Mode::Edit;
        ide.note("stopped");
    }

    if let Some(c) = it.type_char {
        ide.edit_push(c);
    }
    if it.type_back {
        ide.edit_backspace();
    }
    match it.edit {
        Some(Edit::Begin) => {
            if ide.mode.is_play() {
                ide.note("stop the run (r) to edit");
            } else {
                ide.begin_edit();
            }
        }
        Some(Edit::Commit) => {
            if let Some((row, text)) = ide.take_edit() {
                if let Some(Some(t)) = row_map.get(row) {
                    if let Err(e) = edit::commit(vm, t, &text) {
                        ide.note(format!("edit: {e:#}"));
                    }
                }
            }
        }
        Some(Edit::Cancel) => {
            ide.take_edit();
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    /// Edit-mode regression for story-65: the touch/pointer work in
    /// `se-tui::term` and `se-cli::touch` only adds `Keys.mouse*`
    /// alongside `Intent::click` (term.rs still sets it on every
    /// `MouseEventKind::Down`). A click must keep moving IDE focus here.
    #[test]
    fn edit_mode_click_still_changes_panel_focus() {
        let areas = Areas {
            rects: [
                (Panel::Hierarchy, Rect { x: 0, y: 1, width: 20, height: 24 }),
                (Panel::Viewport, Rect { x: 20, y: 1, width: 60, height: 24 }),
                (Panel::Inspector, Rect { x: 80, y: 1, width: 20, height: 24 }),
                (Panel::Project, Rect { x: 0, y: 25, width: 30, height: 10 }),
                (Panel::Terminal, Rect { x: 30, y: 25, width: 70, height: 10 }),
            ],
        };
        let mut ide = Ide::default();
        assert_eq!(ide.mode, Mode::Edit, "IDE opens in Edit mode");
        ide.focused = Panel::Viewport;

        let wit = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../se-contract/tests/game1.wit");
        let g = se_contract::generate(&wit).expect("fixture contract generates");
        let mut vm = Vm::new(&g.module, 1).expect("fixture contract loads");

        // A click inside the Inspector rect (85, 5) must move focus there,
        // same as before touch's pointer state machine existed.
        let it = Intent {
            click: Some((85, 5)),
            ..Default::default()
        };
        apply(it, &mut ide, &mut vm, &[], &areas);

        assert_eq!(ide.focused, Panel::Inspector);
    }
}
