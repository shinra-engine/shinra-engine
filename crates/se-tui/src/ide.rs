//! The IDE, in the layout the original had.
//!
//! ```text
//!  File  Edit  View  Run                                    game 1/3: game1
//! ┌Hierarchy───┐┌Viewport────────────────────────┐┌Inspector───────────────┐
//! ├────────────┤├────────────────────────────────┤├────────────────────────┤
//! └────────────┘└────────────────────────────────┘└────────────────────────┘
//! ┌Project─────────────┐┌Terminal──────────────────────────────────────────┐
//! └────────────────────┘└──────────────────────────────────────────────────┘
//! ```
//!
//! The viewport is the presented buffer drawn as half blocks, so the same
//! frame the engine would hand a window goes here instead. Everything around
//! it is a view of host state and owns none of it.

use crate::present::Pixels;
use crate::log::Level;
use crate::state::{Ide, Panel};
use crate::textart::{self, Frame as ArtFrame};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph};

const ACCENT: Color = Color::Indexed(81);
const DIM: Color = Color::Indexed(242);
const OK: Color = Color::Indexed(250);
const ERR: Color = Color::Indexed(203);

fn panel(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(if focused { BorderType::Thick } else { BorderType::Plain })
        .border_style(Style::default().fg(if focused { ACCENT } else { DIM }))
        .title(Span::styled(
            title,
            Style::default().fg(if focused { ACCENT } else { DIM }),
        ))
}

/// `File Edit View Run` on the left, the deck position on the right.
fn menu_bar(f: &mut Frame, area: Rect, ide: &Ide) {
    let left = Line::from(vec![
        Span::styled(" File ", Style::default().fg(OK)),
        Span::styled(" Edit ", Style::default().fg(OK)),
        Span::styled(" View ", Style::default().fg(OK)),
        Span::styled(" Run ", Style::default().fg(OK)),
        Span::styled(
            format!("   {} ", ide.mode.label()),
            Style::default()
                .fg(if ide.mode.is_play() { Color::Indexed(114) } else { Color::Indexed(203) })
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(left), area);
    let label = ide.game_label();
    let w = label.chars().count() as u16;
    if area.width > w + 2 {
        let right = Rect { x: area.x + area.width - w - 1, y: area.y, width: w, height: 1 };
        f.render_widget(
            Paragraph::new(Span::styled(
                label,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )),
            right,
        );
    }
}

/// Draw the presented frame into `area` using the current glyph set.
///
/// This is the original engine's `textart` renderer, not a half-block
/// substitute: `Mixed` and `Braille` pack 2x4 subpixels into a cell, so the
/// viewport carries four times the vertical detail a `▀` can.
fn viewport(f: &mut Frame, area: Rect, px: Option<&Pixels>, ide: &Ide) {
    let Some(px) = px else { return };
    let cells = textart::image_to_cells(
        &ArtFrame { width: px.width, height: px.height, rgba: px.rgba },
        ide.art,
        textart::DEFAULT_THRESHOLD,
    );
    let buf = f.buffer_mut();
    for row in 0..area.height {
        let Some(line) = cells.get(row as usize) else { break };
        for col in 0..area.width {
            let Some(c) = line.get(col as usize) else { break };
            if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
                cell.set_char(c.ch)
                    .set_fg(Color::Rgb(c.rgb[0], c.rgb[1], c.rgb[2]));
            }
        }
    }
}

fn list<'a>(items: Vec<ListItem<'a>>, block: Block<'a>) -> List<'a> {
    List::new(items).block(block)
}

/// Where each panel was drawn, so a mouse click can be turned into a focus.
#[derive(Default, Clone, Copy)]
pub struct Areas {
    pub rects: [(Panel, Rect); 5],
}

impl Areas {
    /// The viewport's drawable area in cells, borders excluded. This is the
    /// number the framebuffer has to be sized against.
    pub fn viewport_inner(&self) -> Option<(u16, u16)> {
        self.rects
            .iter()
            .find(|(p, _)| *p == Panel::Viewport)
            .map(|(_, r)| (r.width.saturating_sub(2), r.height.saturating_sub(2)))
    }

    pub fn panel_at(&self, col: u16, row: u16) -> Option<Panel> {
        self.rects
            .iter()
            .find(|(_, r)| {
                col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
            })
            .map(|(p, _)| *p)
    }
}

pub fn draw(f: &mut Frame, ide: &Ide, px: Option<&Pixels>) -> Areas {
    let root = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Percentage(30),
    ])
    .split(f.area());

    menu_bar(f, root[0], ide);

    let mid = Layout::horizontal([
        Constraint::Percentage(20),
        Constraint::Percentage(60),
        Constraint::Percentage(20),
    ])
    .split(root[1]);

    // --- Hierarchy: what exists -------------------------------------------
    let mut nodes: Vec<ListItem> = Vec::new();
    for (i, n) in ide.hierarchy.iter().enumerate() {
        let sel = i == ide.selected;
        let open = ide.is_expanded(n);
        // A leaf gets a space where the twisty would be, so labels stay in one
        // column whether or not anything can be unfolded.
        let twisty = match (n.components.is_empty(), open) {
            (true, _) => " ",
            (false, true) => "▾",
            (false, false) => "▸",
        };
        nodes.push(ListItem::new(Line::from(vec![
            Span::styled(if sel { "› " } else { "  " }, Style::default().fg(ACCENT)),
            Span::styled(format!("{twisty} "), Style::default().fg(DIM)),
            Span::styled(
                n.label.clone(),
                if sel {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(OK)
                },
            ),
        ])));
        if open {
            for c in &n.components {
                nodes.push(ListItem::new(Line::from(vec![
                    Span::raw("      "),
                    Span::styled(c.clone(), Style::default().fg(DIM)),
                ])));
            }
        }
    }
    f.render_widget(
        list(nodes, panel("Hierarchy", ide.focused == Panel::Hierarchy)),
        mid[0],
    );

    // --- Viewport: the presented buffer ------------------------------------
    let vp = panel("Viewport", ide.focused == Panel::Viewport);
    let inner = vp.inner(mid[1]);
    f.render_widget(vp, mid[1]);
    viewport(f, inner, px, ide);

    // --- Inspector: the selected entity's components -----------------------
    let on_inspector = ide.focused == Panel::Inspector;
    let rows: Vec<ListItem> = ide
        .inspector
        .iter()
        .enumerate()
        .map(|(i, (k, v))| {
            let here = on_inspector && i == ide.inspector_sel;
            // While a row is being typed into it shows the buffer and a caret,
            // not the value underneath — the two disagree until Enter, and
            // showing the old one would make the edit look like it was lost.
            let (text, style) = match (&ide.editing, here) {
                (Some(buf), true) => {
                    (format!("{buf}▏"), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
                }
                (_, true) => (v.clone(), Style::default().fg(ACCENT)),
                _ => (v.clone(), Style::default().fg(OK)),
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    if here { "›" } else { " " }.to_string(),
                    Style::default().fg(ACCENT),
                ),
                Span::styled(k.clone(), Style::default().fg(DIM)),
                Span::styled(text, style),
            ]))
        })
        .collect();
    f.render_widget(
        list(rows, panel("Inspector", ide.focused == Panel::Inspector)),
        mid[2],
    );

    // --- Project and Terminal ----------------------------------------------
    let bot =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).split(root[2]);

    let tree: Vec<ListItem> = ide
        .project
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if ide.focused == Panel::Project && ide.project_pick.get(i) == Some(&true) && i == ide.project_sel {
                ListItem::new(Span::styled(format!("▸{l}"), Style::default().fg(ACCENT)))
            } else {
                ListItem::new(Span::styled(l.clone(), Style::default().fg(OK)))
            }
        })
        .collect();
    f.render_widget(list(tree, panel("Project", ide.focused == Panel::Project)), bot[0]);

    let term_block = panel("Terminal", ide.focused == Panel::Terminal);
    let room = term_block.inner(bot[1]).height as usize;
    let lines: Vec<Line> = ide
        .log
        .iter()
        .rev()
        .take(room)
        .rev()
        .map(|l| {
            let fg = match l.level {
                Level::Info => DIM,
                Level::Error => ERR,
            };
            Line::from(Span::styled(l.text.clone(), Style::default().fg(fg)))
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(term_block), bot[1]);

    Areas {
        rects: [
            (Panel::Hierarchy, mid[0]),
            (Panel::Viewport, mid[1]),
            (Panel::Inspector, mid[2]),
            (Panel::Project, bot[0]),
            (Panel::Terminal, bot[1]),
        ],
    }
}

/// Status line the runner prints under the panels when there is room.
pub const HELP: &str = " r run/stop · n next game · m glyphs · tab panel · ctrl+h/v/i/f/t focus · q quit · F5 save · F9 load";

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn an_error_draws_red_one_line_at_a_time() {
        let mut ide = Ide::default();
        ide.error("error[E0425]: cannot find value `x`\n --> game/game2.rs:10:5");
        let mut term = ratatui::Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| {
            draw(f, &ide, None);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let w = buf.area.width as usize;
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        let at = text.find("error[E0425]").expect("error line drawn");
        assert!(text.contains("--> game/game2.rs:10:5"));
        let i = text[..at].chars().count();
        let cell = &buf[((i % w) as u16, (i / w) as u16)];
        assert_eq!(cell.fg, Color::Indexed(203));
    }
}
