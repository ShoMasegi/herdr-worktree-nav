//! Drawing the branches picker: the repository step, the branch step, and the destination
//! step with its preview.

use super::*;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::domain::model::RepoNode;
use crate::domain::order::Order;
use crate::domain::preview::{Preview, PreviewPane};
use crate::domain::resolve::{BranchEntry, BranchState};
use crate::domain::rows::abbreviate;
use crate::port::LayoutRect;
use crate::ui::branches::{Activity, BranchesState, Step};
use crate::ui::diagram::{Fit, Frame as DiagramFrame};
use crate::ui::theme::Theme;
use crate::ui::words;

const HELP_REPO: &[&str] = &[
    "\u{21b5} branches  j/k move  / search  \u{21e5} panes  q close",
    "\u{21b5} branches  / search  \u{21e5} panes  q close",
    "\u{21b5} branches  esc close",
];

const HELP_REPO_SEARCH: &[&str] = &[
    "\u{21b5} branches  ctrl+u clear  esc cancel  \u{2191}\u{2193} move",
    "\u{21b5} branches  esc cancel",
];

/// The branch step, when Esc has a repository list to go back to. As in the panes view, the
/// other view outranks a way of moving around this one, so `Tab` survives to the
/// second-to-last rung.
const HELP_BRANCH_BACK: &[&str] = &[
    "\u{21b5} choose  j/k move  / search  n new branch  f fetch  i order  shift+i reverse  \u{21e5} panes  esc back  q close",
    "\u{21b5} choose  j/k move  / search  n new branch  f fetch  i order  \u{21e5} panes  esc back  q close",
    "\u{21b5} choose  / search  n new branch  f fetch  \u{21e5} panes  esc back",
    "\u{21b5} choose  / search  n new branch  \u{21e5} panes  esc back",
    "\u{21b5} choose  / search  \u{21e5} panes  esc back",
    "\u{21b5} choose  esc",
];

/// The same, with only one repository open: Esc has nowhere to go but out.
const HELP_BRANCH: &[&str] = &[
    "\u{21b5} choose  j/k move  / search  n new branch  f fetch  i order  shift+i reverse  \u{21e5} panes  q close",
    "\u{21b5} choose  j/k move  / search  n new branch  f fetch  i order  \u{21e5} panes  q close",
    "\u{21b5} choose  / search  n new branch  f fetch  \u{21e5} panes  q close",
    "\u{21b5} choose  / search  n new branch  \u{21e5} panes  q close",
    "\u{21b5} choose  / search  \u{21e5} panes  q close",
    "\u{21b5} choose  esc",
];

/// While the name of a new branch is being typed.
const HELP_NAME: &[&str] = &[
    "\u{21b5} next  ctrl+u clear  esc back",
    "\u{21b5} next  esc back",
];

const HELP_DESTINATION: &[&str] = &[
    "\u{21b5} open here  \u{2191}\u{2193} move  esc back  q close",
    "\u{21b5} open  esc back",
];

/// While a step that can still be abandoned is running.
const HELP_WORKING_STOPPABLE: &[&str] = &["ctrl+c stop", "ctrl+c"];

/// While one that cannot: stopping now would leave a workspace nobody moved.
const HELP_WORKING: &[&str] = &["working\u{2026}"];

const HELP_FAILED: &[&str] = &["\u{21b5} close  esc close", "\u{21b5} close"];

/// Searching either list: the `Ctrl-` forms keep working here, which is how an order or a
/// fetch is reached without abandoning what has been typed.
const HELP_BRANCH_SEARCH: &[&str] = &[
    "\u{21b5} choose  ctrl+u clear  esc cancel  \u{2191}\u{2193} move  ctrl+f fetch  ctrl+o order  ctrl+r reverse",
    "\u{21b5} choose  ctrl+u clear  esc cancel  \u{2191}\u{2193} move  ctrl+f fetch",
    "\u{21b5} choose  esc cancel",
];

/// Widest branch name to give a column to. One very long name must not squeeze out the
/// state and the pull request beside it.
const MAX_BRANCH_COLUMN: usize = 40;

/// Fits "checked out", the longest state word, and is a floor rather than a fixed width:
/// `gone` goes inside this column so that what follows — the pull request, or the commit
/// subject — stays lined up down the list.
const STATE_COLUMN: usize = 12;

/// Widest repository name to give a column to, for the same reason as the branch column.
const MAX_REPO_COLUMN: usize = 40;

/// Fits "12 worktrees, 34 panes".
const COUNT_COLUMN: usize = 22;

/// The `/` lights up when the search field has the keyboard, exactly as it does in the
/// panes view: it is the only thing on screen that says which of the two modes you are in.
fn prompt_style(state: &BranchesState, theme: &Theme) -> Style {
    if state.is_filtering() {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.dim()
    }
}

pub fn draw(frame: &mut Frame, state: &BranchesState, theme: &Theme) {
    let Some(panel) = layout(frame) else {
        return;
    };

    match state.step() {
        Step::Repo => {
            frame.render_widget(
                repo_search_line(state, theme, panel.search.width),
                panel.search,
            );
            render_rule(frame, panel.rule, theme);
            render_repo_rows(frame, state, theme, panel.body);
            render_detail(frame, &state.repo_detail(), theme, panel.detail);
            let variants = if state.is_filtering() {
                HELP_REPO_SEARCH
            } else {
                HELP_REPO
            };
            frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
        }
        Step::Branch => {
            frame.render_widget(
                branch_search_line(state, theme, panel.search.width),
                panel.search,
            );
            // The rule the other steps draw plain is a heading here: a list of branches
            // means nothing without knowing whose branches they are.
            render_detail(frame, &state.repo_heading(), theme, panel.rule);
            render_branch_rows(frame, state, theme, panel.body);
            render_detail(frame, &state.detail(), theme, panel.detail);
            let variants = match (state.is_filtering(), state.has_repo_step()) {
                (true, _) => HELP_BRANCH_SEARCH,
                (false, true) => HELP_BRANCH_BACK,
                (false, false) => HELP_BRANCH,
            };
            frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
        }
        // The list stays under the prompt because the base is on it: taking it away would
        // ask what to call the new branch without showing what it is cut from.
        Step::Name => {
            frame.render_widget(name_prompt(state, theme, panel.search.width), panel.search);
            render_detail(frame, &state.repo_heading(), theme, panel.rule);
            render_branch_rows(frame, state, theme, panel.body);
            render_detail(frame, &state.detail(), theme, panel.detail);
            frame.render_widget(footer(HELP_NAME, theme, panel.footer.width), panel.footer);
        }
        Step::Destination => {
            frame.render_widget(
                destination_prompt(state, theme, panel.search.width),
                panel.search,
            );
            render_rule(frame, panel.rule, theme);
            let (list, preview) = destination_areas(panel.body);
            render_destination_rows(frame, state, theme, list);
            if let Some(preview) = preview {
                render_preview(frame, &state.preview(), theme, preview);
            }
            render_detail(frame, &state.destination_detail(), theme, panel.detail);
            let variants = match state.activity() {
                Activity::Choosing => HELP_DESTINATION,
                Activity::Working { stage, .. } if stage.interruptible() => HELP_WORKING_STOPPABLE,
                Activity::Working { .. } => HELP_WORKING,
                Activity::Failed { .. } => HELP_FAILED,
            };
            frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
        }
    }
}

fn branch_search_line(state: &BranchesState, theme: &Theme, width: u16) -> Paragraph<'static> {
    let mut spans = vec![Span::styled(" / ", prompt_style(state, theme))];
    if let Some(message) = state.message() {
        spans.push(Span::styled(
            message.to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    } else if state.query().is_empty() {
        if !state.is_filtering() {
            spans.push(Span::styled("search branches", theme.dim()));
        }
    } else {
        spans.push(Span::raw(state.query().to_string()));
    }
    if state.is_filtering() {
        spans.push(Span::styled("\u{2588}", theme.dim()));
    }
    // Anything being waited for carries the spinner, so a picker that is busy never looks
    // like one that is stuck. A fetch says so louder than the listing, because it was asked
    // for.
    let (waiting, style) = if state.is_fetching() {
        ("fetching origin", Style::default().fg(theme.accent))
    } else if state.is_loading() {
        ("reading the remote", theme.dim())
    } else {
        ("", theme.dim())
    };
    if !waiting.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(spinner(state.frame()), style));
        spans.push(Span::styled(format!(" {waiting}\u{2026}"), style));
    }

    // The order sits beside the count, where the eye already goes to read how long the list
    // is, and takes the accent once it is no longer the default: there is otherwise no way
    // to tell without reading the rows.
    let order = format!("\u{21c5} {}", words::order(state.order()));
    let count = count_of(state.rows().len(), "branch", "branches");
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let right = order.chars().count() + ORDER_GAP + count.chars().count();
    let pad = (width as usize).saturating_sub(used + right + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(
        order,
        if state.order() == Order::default() {
            theme.dim()
        } else {
            Style::default().fg(theme.accent)
        },
    ));
    spans.push(Span::raw(" ".repeat(ORDER_GAP)));
    spans.push(Span::styled(count, theme.dim()));
    Paragraph::new(Line::from(spans))
}

/// Between the order and the count on the search line.
const ORDER_GAP: usize = 3;

/// `+ new branch from <base>: <name>`, in place of the search line.
///
/// The base is named rather than only highlighted in the list: it is the whole difference
/// between this and the offer to create, which starts from `HEAD`.
fn name_prompt(state: &BranchesState, theme: &Theme, width: u16) -> Paragraph<'static> {
    let (base, typed) = state.naming().unwrap_or(("", ""));
    let mut spans = vec![
        Span::styled(
            " + ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("new branch from ", theme.dim()),
        Span::styled(base.to_string(), Style::default().fg(theme.accent)),
        Span::styled(": ", theme.dim()),
    ];
    match state.message() {
        // The reason a name was refused belongs where the name is, not under the list.
        Some(message) => spans.push(Span::styled(
            message.to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        None => {
            spans.push(Span::raw(typed.to_string()));
            spans.push(Span::styled("\u{2588}", theme.dim()));
        }
    }

    let count = count_of(state.rows().len(), "branch", "branches");
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let pad = (width as usize).saturating_sub(used + count.chars().count() + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(count, theme.dim()));
    Paragraph::new(Line::from(spans))
}

fn repo_search_line(state: &BranchesState, theme: &Theme, width: u16) -> Paragraph<'static> {
    let mut spans = vec![Span::styled(" / ", prompt_style(state, theme))];
    if let Some(message) = state.message() {
        spans.push(Span::styled(
            message.to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    } else if state.repo_query().is_empty() {
        if !state.is_filtering() {
            spans.push(Span::styled("search repositories", theme.dim()));
        }
    } else {
        spans.push(Span::raw(state.repo_query().to_string()));
    }
    if state.is_filtering() {
        spans.push(Span::styled("\u{2588}", theme.dim()));
    }

    let count = count_of(state.repo_rows().len(), "repository", "repositories");
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let pad = (width as usize).saturating_sub(used + count.chars().count() + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(count, theme.dim()));
    Paragraph::new(Line::from(spans))
}

fn render_repo_rows(frame: &mut Frame, state: &BranchesState, theme: &Theme, area: Rect) {
    let rows = state.repo_rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" no repositories", theme.dim()))),
            area,
        );
        return;
    }

    let width = area.width as usize;
    let name_column = rows
        .iter()
        .map(|row| row.repo.display_name.chars().count())
        .max()
        .unwrap_or(0)
        .min(MAX_REPO_COLUMN);

    let viewport = area.height as usize;
    let scroll = scroll_offset(state.repo_cursor(), rows.len(), viewport);
    let end = rows.len().min(scroll + viewport);

    for (offset, row) in rows[scroll..end].iter().enumerate() {
        let selected = scroll + offset == state.repo_cursor();
        let base = if selected {
            theme.selected()
        } else {
            Style::default()
        };
        // The same mark the panes view puts on the row the session is focused on.
        let gutter = if row.is_origin { " \u{25c6} " } else { "   " };
        let gutter_style = if selected {
            base
        } else if row.is_origin {
            Style::default().fg(theme.accent)
        } else {
            theme.dim()
        };
        let quiet = if selected { base } else { theme.dim() };

        let mut spans = vec![
            Span::styled(gutter, gutter_style),
            Span::styled(
                pad(&truncate(&row.repo.display_name, name_column), name_column),
                if selected {
                    base.add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ),
            Span::raw("  "),
            Span::styled(pad(&counts(row.repo), COUNT_COLUMN), quiet),
        ];

        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let path = middle_elide(
            &abbreviate(&row.repo.repo_root, state.home()),
            width.saturating_sub(used + 2),
        );
        if !path.is_empty() {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(path, quiet));
        }

        let rect = Rect::new(area.x, area.y + offset as u16, area.width, 1);
        frame.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    render_scrollbar(frame, scroll, rows.len(), viewport, theme, area);
}

/// How much of a repository is open: what a name alone cannot tell you.
fn counts(repo: &RepoNode) -> String {
    let worktrees = count_of(repo.worktrees.len(), "worktree", "worktrees");
    let panes: usize = repo.worktrees.iter().map(|w| w.panes.len()).sum();
    format!("{worktrees}, {}", count_of(panes, "pane", "panes"))
}

fn destination_prompt(state: &BranchesState, theme: &Theme, width: u16) -> Paragraph<'static> {
    match state.activity() {
        Activity::Choosing => {}
        // The step replaces the question, because the question has been answered.
        Activity::Working { stage } => {
            return Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(spinner(state.frame()), Style::default().fg(theme.accent)),
                Span::raw(" "),
                Span::styled(
                    words::stage(stage),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled("\u{2026}", theme.dim()),
            ]));
        }
        Activity::Failed { stage, error } => {
            let head = format!(" \u{d7} {}: ", words::stage(stage));
            // Cut out of the middle rather than the end: git puts the command it ran first
            // and its actual complaint last, and the complaint is the point.
            let error = middle_elide(
                error,
                (width as usize).saturating_sub(head.chars().count() + 1),
            );
            return Paragraph::new(Line::from(vec![
                Span::styled(
                    " \u{d7} ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{}: ", words::stage(stage)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(error, Style::default().fg(Color::Red)),
            ]));
        }
    }
    if let Some(message) = state.message() {
        return Paragraph::new(Line::from(Span::styled(
            format!(" {message}"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
    }
    let name = state
        .chosen()
        .map(|c| c.name().to_string())
        .unwrap_or_default();
    Paragraph::new(Line::from(vec![
        Span::styled(" where should ", theme.dim()),
        Span::styled(
            name,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" go?", theme.dim()),
    ]))
}

fn render_branch_rows(frame: &mut Frame, state: &BranchesState, theme: &Theme, area: Rect) {
    let rows = state.rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" no branches", theme.dim()))),
            area,
        );
        return;
    }

    let width = area.width as usize;
    let name_column = rows
        .iter()
        .map(|entry| entry.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(MAX_BRANCH_COLUMN);
    // The longest state actually in this list, plus the space that keeps it off whatever
    // follows, and never narrower than the floor. Only a branch whose upstream is gone needs
    // more than the floor, so only the lists that have one pay for it.
    let state_column = rows
        .iter()
        .map(|entry| branch_state_label(entry).chars().count() + 1)
        .max()
        .unwrap_or(0)
        .max(STATE_COLUMN);

    let viewport = area.height as usize;
    let scroll = scroll_offset(state.cursor(), rows.len(), viewport);
    let end = rows.len().min(scroll + viewport);

    for (offset, entry) in rows[scroll..end].iter().enumerate() {
        let selected = scroll + offset == state.cursor();
        let base = if selected {
            theme.selected()
        } else {
            Style::default()
        };
        let (glyph, glyph_style) = branch_glyph(entry, theme);
        let glyph_style = if selected { base } else { glyph_style };

        let mut spans = vec![
            Span::styled("   ", base),
            Span::styled(glyph, glyph_style),
            Span::raw(" "),
            Span::styled(
                pad(&truncate(&entry.name, name_column), name_column),
                if selected {
                    base.add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ),
            Span::raw("  "),
            Span::styled(
                pad(&branch_state_label(entry), state_column),
                if selected { base } else { theme.dim() },
            ),
        ];

        // A pull request is the most useful thing to know about a branch, so it wins the
        // remaining space over the commit subject.
        let detail = match (&entry.pull_request, &entry.subject) {
            (Some(pr), _) => Some(format!(
                "#{} {}{}",
                pr.number,
                pr.title,
                if pr.is_draft { " (draft)" } else { "" }
            )),
            (None, Some(subject)) => Some(subject.clone()),
            (None, None) => None,
        };
        if let Some(detail) = detail {
            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let text = truncate(&detail, width.saturating_sub(used + 2));
            if !text.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    text,
                    if selected { base } else { theme.dim() },
                ));
            }
        }

        let rect = Rect::new(area.x, area.y + offset as u16, area.width, 1);
        frame.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
    }
    render_scrollbar(frame, scroll, rows.len(), viewport, theme, area);
}

fn render_destination_rows(frame: &mut Frame, state: &BranchesState, theme: &Theme, area: Rect) {
    // The group name is a left column rather than a heading row, so the cursor index stays
    // a plain index into the destination list and every row lines up.
    let group_column = state
        .destinations()
        .iter()
        .map(|destination| words::destination_group(destination).chars().count())
        .max()
        .unwrap_or(0);

    let viewport = area.height as usize;
    let scroll = scroll_offset(
        state.destination_cursor(),
        state.destinations().len(),
        viewport,
    );
    let end = state.destinations().len().min(scroll + viewport);

    let mut last_group = "";
    for (index, destination) in state.destinations().iter().enumerate() {
        let group = words::destination_group(destination);
        let shown = if group == last_group { "" } else { group };
        last_group = group;
        if index < scroll || index >= end {
            continue;
        }
        let selected = index == state.destination_cursor();
        let base = if selected {
            theme.selected()
        } else {
            Style::default()
        };
        let rect = Rect::new(area.x, area.y + (index - scroll) as u16, area.width, 1);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ", base),
                Span::styled(
                    pad(shown, group_column),
                    if selected { base } else { theme.dim() },
                ),
                Span::raw("  "),
                Span::styled(
                    words::destination(destination),
                    if selected {
                        base.add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
            ]))
            .style(base),
            rect,
        );
    }
    render_scrollbar(
        frame,
        scroll,
        state.destinations().len(),
        viewport,
        theme,
        area,
    );
}

/// Widest the destination list is allowed to get; past this the preview is the better use
/// of the space.
const DESTINATION_LIST_WIDTH: u16 = 46;

/// Below this there is no room for a diagram worth looking at, so the list takes it all.
const MIN_PREVIEW_WIDTH: u16 = 28;

/// Split the body into the destination list and the preview beside it.
fn destination_areas(body: Rect) -> (Rect, Option<Rect>) {
    let list_width = DESTINATION_LIST_WIDTH.min(body.width * 45 / 100);
    if body.width.saturating_sub(list_width) < MIN_PREVIEW_WIDTH {
        return (body, None);
    }
    let list = Rect::new(body.x, body.y, list_width, body.height);
    // Two columns of gutter, so the diagram is not touching the labels.
    let preview = Rect::new(
        body.x + list_width + 2,
        body.y,
        body.width - list_width - 2,
        body.height,
    );
    (list, Some(preview))
}

/// Draw what the chosen tab will look like once the branch's pane is in it.
fn render_preview(frame: &mut Frame, preview: &Preview, theme: &Theme, area: Rect) {
    match preview {
        Preview::Unavailable => {}
        Preview::Blocked { at, reason } => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(words::landing(at), theme.dim())),
                    Line::from(""),
                    Line::from(Span::styled(
                        format!("\u{26a0} {}", words::refusal(*reason)),
                        Style::default().fg(Color::Yellow),
                    )),
                ])
                .wrap(Wrap { trim: true }),
                area,
            );
        }
        Preview::Layout {
            at,
            area: tab,
            panes,
        } => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(words::landing(at), theme.dim()))),
                Rect::new(area.x, area.y, area.width, 1),
            );
            if area.height < 4 {
                return;
            }
            let canvas = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
            render_diagram(frame, *tab, panes, theme, canvas);
        }
    }
}

fn render_diagram(
    frame: &mut Frame,
    tab: LayoutRect,
    panes: &[PreviewPane],
    theme: &Theme,
    canvas: Rect,
) {
    let Some(fit) = Fit::new(tab, canvas.width as usize, canvas.height as usize) else {
        return;
    };
    let (width, height) = fit.size();
    let (offset_x, offset_y) = fit.offset();

    let mut grid = DiagramFrame::new(width, height);
    let mapped: Vec<(LayoutRect, &PreviewPane)> = panes
        .iter()
        .map(|pane| (fit.map(pane.rect), pane))
        .collect();
    for (rect, _) in &mapped {
        grid.add(*rect);
    }

    // The borders first, as one divided rectangle rather than a row of separate boxes.
    for y in 0..height {
        let line: String = (0..width).map(|x| grid.glyph_at(x, y)).collect();
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(line, theme.tree()))),
            Rect::new(
                canvas.x + offset_x as u16,
                canvas.y + (offset_y + y) as u16,
                width as u16,
                1,
            ),
        );
    }

    for (rect, pane) in &mapped {
        // One column in from the border, so text is not touching it.
        let inner = Rect::new(
            canvas.x + offset_x as u16 + rect.x + 2,
            canvas.y + offset_y as u16 + rect.y + 1,
            rect.width.saturating_sub(3),
            rect.height.saturating_sub(2),
        );
        if inner.width == 0 || inner.height == 0 {
            continue;
        }
        let (glyph, glyph_style) = theme.status_glyph(pane.status);
        let name_style = if pane.is_new {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let head = if pane.is_new {
            vec![
                Span::styled("+ ", name_style),
                Span::styled(
                    truncate(
                        pane.label.as_deref().unwrap_or(words::UNNAMED_PANE),
                        inner.width.saturating_sub(2) as usize,
                    ),
                    name_style,
                ),
            ]
        } else {
            vec![
                Span::styled(glyph, glyph_style),
                Span::raw(" "),
                Span::styled(
                    truncate(
                        pane.label.as_deref().unwrap_or(words::UNNAMED_PANE),
                        inner.width.saturating_sub(2) as usize,
                    ),
                    name_style,
                ),
            ]
        };

        if inner.height >= 2 && !pane.id.is_empty() {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(head),
                    Line::from(Span::styled(
                        truncate(&pane.id, inner.width as usize),
                        theme.dim(),
                    )),
                ]),
                inner,
            );
        } else {
            // One line only, so the name and the id share it.
            let mut spans = head;
            if !pane.id.is_empty() {
                let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                let room = (inner.width as usize).saturating_sub(used + 2);
                if room > 0 {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(truncate(&pane.id, room), theme.dim()));
                }
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), inner);
        }
    }
}

/// A glyph and colour for what a branch currently is, in the same visual language as the
/// agent status glyphs.
fn branch_glyph(entry: &BranchEntry, theme: &Theme) -> (&'static str, Style) {
    match entry.state {
        BranchState::LivePane { .. } => ("\u{25cf}", Style::default().fg(Color::Yellow)),
        BranchState::IdleWorktree { .. } => ("\u{25cb}", Style::default().fg(Color::Green)),
        BranchState::LocalRef => ("\u{b7}", theme.dim()),
        BranchState::RemoteOnly => ("\u{2193}", Style::default().fg(Color::Blue)),
        BranchState::New => (
            "+",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

fn branch_state_label(entry: &BranchEntry) -> String {
    let state = match entry.state {
        BranchState::LivePane { .. } => "running",
        BranchState::IdleWorktree { .. } => "checked out",
        BranchState::LocalRef => "local",
        BranchState::RemoteOnly => "remote",
        BranchState::New => "create",
    };
    // What the branch is, and then whether git can still find what it tracks. The second is
    // not a state of its own: a branch whose upstream is gone is still checked out, or still
    // running, and saying only `gone` would drop the half that says where it is.
    match entry.upstream_gone() {
        true => format!("{state} gone"),
        false => state.to_string(),
    }
}

#[cfg(test)]
mod tests {

    use crate::ui::render::fixtures::*;

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::domain::progress::Stage;

    use crate::port::Track;

    use crate::ui::branches::BranchData;

    #[test]
    fn draws_the_repositories_the_session_has_open() {
        insta::assert_snapshot!(branches_screen(&branches_picker(), 92, 12));
    }

    #[test]
    fn a_branch_whose_upstream_is_gone_says_so_beside_what_it_is() {
        // The ordinary end of a merged branch: GitHub deleted the head, a pruning fetch
        // noticed, and the local branch and its checkout are all that is left.
        let mut state = branches_picker();
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let mut data = branch_data();
        data.local_refs[1].track = Some(Track::Gone);
        data.local_refs[3].track = Some(Track::Gone);
        state.set_data(data);
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    #[test]
    fn draws_branches_in_the_same_chrome_as_the_panes_view() {
        insta::assert_snapshot!(branches_screen(&branches_state(), 92, 12));
    }

    /// Entered from the second repository rather than the first, so a heading wired to
    /// whichever one the picker opened on would show.
    #[test]
    fn names_the_repository_above_the_list_and_not_on_every_row() {
        let mut state = branches_picker();
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        state.set_data(branch_data());
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    #[test]
    fn draws_a_spinner_beside_whatever_it_is_waiting_for() {
        let mut state = branches_state();
        state.set_data(BranchData {
            fetching: true,
            ..branch_data()
        });
        state.tick();
        insta::assert_snapshot!(branches_screen(&state, 110, 8));
    }

    #[test]
    fn draws_the_listing_it_starts_with_as_a_quieter_wait_of_the_same_shape() {
        let mut state = branches_state();
        state.set_data(BranchData {
            loading: true,
            ..branch_data()
        });
        insta::assert_snapshot!(branches_screen(&state, 110, 8));
    }

    #[test]
    fn the_branch_search_field_hides_its_placeholder_too() {
        let mut state = branches_state();
        state.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 92, 6));
    }

    #[test]
    fn draws_the_order_beside_the_count_and_reorders_the_list_to_match() {
        let mut state = branches_state();
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    #[test]
    fn draws_the_offer_to_create_a_branch_that_does_not_exist() {
        let mut state = branches_state();
        search(&mut state, "feat/brand-new");
        insta::assert_snapshot!(branches_screen(&state, 92, 10));
    }

    #[test]
    fn draws_the_step_it_is_on_instead_of_the_question_it_already_asked() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        state.start_working(Stage::Fetching {
            remote: "origin".into(),
            branch: "chore/deps".into(),
        });
        insta::assert_snapshot!(branches_screen(&state, 110, 14));
    }

    #[test]
    fn draws_a_failure_where_the_step_was() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        state.start_working(Stage::Fetching {
            remote: "origin".into(),
            branch: "chore/deps".into(),
        });
        state.fail("could not read from remote repository".into());
        insta::assert_snapshot!(branches_screen(&state, 110, 14));
    }

    #[test]
    fn draws_a_warning_instead_of_a_diagram_for_a_zoomed_tab() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        // Move to the zoomed tab, which is the last destination in this fixture.
        state.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 110, 14));
    }

    #[test]
    fn draws_the_destination_preview_at_a_realistic_size() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 110, 22));
    }

    #[test]
    fn draws_the_prompt_for_a_branch_started_from_the_one_under_the_cursor() {
        let mut state = branches_state();
        state.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        for c in "feat/login-v2".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    /// A name git would take is not the same as a name this repository has room for.
    #[test]
    fn draws_the_reason_a_name_was_refused_where_the_name_is() {
        let mut state = branches_state();
        state.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        for c in "main".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    #[test]
    fn draws_the_destination_step_with_split_here_selected() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }
}
