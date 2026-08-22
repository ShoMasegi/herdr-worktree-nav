//! Drawing the picker.
//!
//! Rendering is a function of state, so the whole screen is covered by snapshot tests over
//! a `TestBackend` buffer rather than by looking at a terminal.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::domain::rows::{Row, RowRef};
use crate::ui::state::PanesState;
use crate::ui::theme;

/// Which picker is on screen. Both share the header, filter line, and help line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Panes,
    Branches,
}

/// Help for the panes view, longest first. The first one that fits the width is used, so a
/// narrow pane shows fewer keys rather than a line cut off mid-word.
const HELP_PANES: &[&str] = &[
    "\u{21b5} jump  n new pane  \u{21e5} branches  / filter  h other  r reload  q quit",
    "\u{21b5} jump  n new  \u{21e5} branches  / filter  r reload",
    "\u{21b5} jump  \u{21e5} branches  / filter",
    "\u{21b5} jump",
];
const HELP_FILTER: &[&str] = &[
    "\u{21b5} keep filter  esc clear  \u{2191}\u{2193} move",
    "\u{21b5} keep  esc clear",
];

pub fn draw(frame: &mut Frame, state: &PanesState, mode: Mode) {
    let [header, body, filter, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(header_line(mode), header);
    draw_rows(frame, state, body);
    frame.render_widget(filter_line(state), filter);

    let variants = if state.is_filtering() {
        HELP_FILTER
    } else {
        HELP_PANES
    };
    let help_text = fit(variants, help.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(help_text, theme::dim()))),
        help,
    );
}

/// Paths and pane ids are right-aligned, but not to the far edge of a very wide pane: past
/// roughly this column the eye can no longer connect a label to the value beside it.
const ALIGN_CAP: usize = 96;

/// The longest variant that fits, or the shortest one when none does.
fn fit<'a>(variants: &[&'a str], width: usize) -> &'a str {
    variants
        .iter()
        .find(|text| text.chars().count() <= width)
        .copied()
        .unwrap_or_else(|| variants.last().copied().unwrap_or_default())
}

fn header_line(mode: Mode) -> Paragraph<'static> {
    let (panes, branches) = match mode {
        Mode::Panes => (theme::header_active(), theme::header_inactive()),
        Mode::Branches => (theme::header_inactive(), theme::header_active()),
    };
    Paragraph::new(Line::from(vec![
        Span::styled(" Panes ", panes),
        Span::raw(" "),
        Span::styled(" Branches ", branches),
    ]))
}

fn filter_line(state: &PanesState) -> Paragraph<'static> {
    let query = state.query();
    if state.is_filtering() {
        // A block cursor, since the real one is parked wherever ratatui left it.
        Paragraph::new(Line::from(vec![
            Span::styled("/", theme::repo_style()),
            Span::raw(query.to_string()),
            Span::styled("\u{2588}", theme::dim()),
        ]))
    } else if let Some(message) = state.message() {
        Paragraph::new(Line::from(Span::styled(
            message.to_string(),
            theme::header_active(),
        )))
    } else if query.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            "press / to filter".to_string(),
            theme::dim(),
        )))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("/", theme::dim()),
            Span::styled(query.to_string(), theme::dim()),
        ]))
    }
}

fn draw_rows(frame: &mut Frame, state: &PanesState, area: Rect) {
    if state.rows().is_empty() {
        let text = if state.query().is_empty() {
            "no repositories open"
        } else {
            "nothing matches"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, theme::dim())))
                .block(Block::default().borders(Borders::NONE)),
            area,
        );
        return;
    }

    let width = area.width as usize;
    let items: Vec<ListItem> = state
        .rows()
        .iter()
        .map(|row| ListItem::new(row_line(row, state, width)))
        .collect();

    let mut list_state = ListState::default().with_selected(Some(state.cursor()));
    frame.render_stateful_widget(
        List::new(items).highlight_style(theme::selected()),
        area,
        &mut list_state,
    );
}

fn row_line<'a>(row: &'a Row, state: &'a PanesState, width: usize) -> Line<'a> {
    let mut spans = vec![Span::raw("  ".repeat(row.indent as usize))];

    match row.reference {
        RowRef::Repo(index) => {
            let marker = if state.is_collapsed(index) {
                theme::REPO_COLLAPSED
            } else {
                theme::REPO_EXPANDED
            };
            spans.push(Span::styled(format!("{marker} "), theme::dim()));
            spans.push(Span::styled(row.primary.as_str(), theme::repo_style()));
        }
        RowRef::Worktree(repo, worktree) => {
            let node = &state.tree().repos[repo].worktrees[worktree];
            let marker = if node.is_primary {
                theme::PRIMARY_CHECKOUT
            } else {
                theme::LINKED_CHECKOUT
            };
            spans.push(Span::styled(format!("{marker} "), theme::dim()));
            spans.push(Span::styled(row.primary.as_str(), theme::worktree_style()));
            if node.is_idle() {
                spans.push(Span::styled("  no pane", theme::dim()));
            }
        }
        RowRef::Pane(..) | RowRef::Ungrouped(_) => {
            let (glyph, style) = row
                .status
                .map(theme::agent_glyph)
                .unwrap_or((" ", Style::default()));
            spans.push(Span::styled(glyph, style));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(row.primary.as_str(), theme::pane_style()));
        }
        RowRef::UngroupedHeader => {
            spans.push(Span::styled(
                format!("\u{2500}\u{2500} {} ", row.primary),
                theme::dim(),
            ));
            return Line::from(spans);
        }
    }

    // The path or pane id, pushed to the right edge and dropped when it will not fit.
    // A repository's path is only shown while it is folded: expanded, the primary worktree
    // row directly beneath already carries it.
    let show_secondary = match row.reference {
        RowRef::Repo(index) => state.is_collapsed(index),
        _ => true,
    };
    if show_secondary && !row.secondary.is_empty() {
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let secondary = row.secondary.chars().count();
        let needed = used + secondary + 2;
        if needed <= width {
            // Align at the cap, but let a long path push past it rather than be dropped.
            let end = ALIGN_CAP.max(needed).min(width);
            spans.push(Span::raw(" ".repeat(end - used - secondary)));
            spans.push(Span::styled(row.secondary.as_str(), theme::dim()));
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;

    use crate::domain::model::{PaneNode, RepoNode, Tree, WorktreeNode};
    use crate::port::AgentStatus;

    fn pane(id: &str, name: &str, status: AgentStatus) -> PaneNode {
        PaneNode {
            pane_id: id.into(),
            workspace_id: id.split(':').next().unwrap().into(),
            tab_id: format!("{}:t1", id.split(':').next().unwrap()),
            display_name: Some(name.into()),
            agent_status: status,
            focused: false,
        }
    }

    fn tree() -> Tree {
        Tree {
            repos: vec![
                RepoNode {
                    repo_key: "/src/app/.git".into(),
                    repo_root: "/src/app".into(),
                    display_name: "me/app".into(),
                    worktrees: vec![
                        WorktreeNode {
                            branch: Some("main".into()),
                            checkout_path: "/src/app".into(),
                            is_primary: true,
                            open_workspace_id: Some("w1".into()),
                            panes: vec![
                                pane("w1:p1", "claude", AgentStatus::Working),
                                pane("w1:p2", "shell", AgentStatus::Unknown),
                            ],
                        },
                        WorktreeNode {
                            branch: Some("feat/login".into()),
                            checkout_path: "/wt/app/feat-login".into(),
                            is_primary: false,
                            open_workspace_id: Some("w2".into()),
                            panes: vec![pane("w2:p1", "codex", AgentStatus::Idle)],
                        },
                        WorktreeNode {
                            branch: Some("fix/crash".into()),
                            checkout_path: "/wt/app/fix-crash".into(),
                            is_primary: false,
                            open_workspace_id: None,
                            panes: vec![],
                        },
                    ],
                },
                RepoNode {
                    repo_key: "/src/site/.git".into(),
                    repo_root: "/src/site".into(),
                    display_name: "me/site".into(),
                    worktrees: vec![WorktreeNode {
                        branch: Some("develop".into()),
                        checkout_path: "/src/site".into(),
                        is_primary: true,
                        open_workspace_id: Some("w3".into()),
                        panes: vec![pane("w3:p1", "claude", AgentStatus::Blocked)],
                    }],
                },
            ],
            ungrouped: vec![pane("w9:p1", "shell", AgentStatus::Unknown)],
        }
    }

    fn screen(state: &PanesState) -> String {
        let mut terminal = Terminal::new(TestBackend::new(72, 16)).unwrap();
        terminal
            .draw(|frame| draw(frame, state, Mode::Panes))
            .unwrap();
        terminal.backend().to_string()
    }

    fn press(state: &mut PanesState, code: KeyCode) {
        state.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn draws_the_repo_worktree_pane_tree() {
        insta::assert_snapshot!(screen(&PanesState::new(tree())));
    }

    #[test]
    fn draws_a_folded_repository() {
        let mut state = PanesState::new(tree());
        press(&mut state, KeyCode::Enter);
        insta::assert_snapshot!(screen(&state));
    }

    #[test]
    fn draws_the_filter_line_while_searching() {
        let mut state = PanesState::new(tree());
        press(&mut state, KeyCode::Char('/'));
        for c in "codex".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        insta::assert_snapshot!(screen(&state));
    }

    #[test]
    fn draws_the_empty_state_when_nothing_matches() {
        let mut state = PanesState::new(tree());
        press(&mut state, KeyCode::Char('/'));
        for c in "zzzz".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        insta::assert_snapshot!(screen(&state));
    }

    #[test]
    fn draws_panes_outside_any_repository_once_revealed() {
        let mut state = PanesState::new(tree());
        press(&mut state, KeyCode::Char('h'));
        insta::assert_snapshot!(screen(&state));
    }

    #[test]
    fn stops_pushing_paths_rightwards_in_a_very_wide_pane() {
        let state = PanesState::new(tree());
        let mut terminal = Terminal::new(TestBackend::new(160, 14)).unwrap();
        terminal
            .draw(|frame| draw(frame, &state, Mode::Panes))
            .unwrap();
        insta::assert_snapshot!(terminal.backend().to_string());
    }

    #[test]
    fn drops_the_trailing_path_rather_than_wrapping_in_a_narrow_terminal() {
        let state = PanesState::new(tree());
        let mut terminal = Terminal::new(TestBackend::new(28, 12)).unwrap();
        terminal
            .draw(|frame| draw(frame, &state, Mode::Panes))
            .unwrap();
        insta::assert_snapshot!(terminal.backend().to_string());
    }
}
