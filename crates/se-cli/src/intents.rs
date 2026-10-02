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
