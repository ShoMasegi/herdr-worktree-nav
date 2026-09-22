//! The two lists the picker starts with: the repositories the session has open, and the
//! branches of whichever one was chosen.
//!
//! Both are a search line over a list of rows, and both are drawn into the same chrome, so
//! the difference between them is what a row says rather than how one is put on the screen.

use crate::ui::render::branches::*;

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

pub(super) fn branch_search_line(
    state: &BranchesState,
    theme: &Theme,
    width: u16,
) -> Paragraph<'static> {
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

pub(super) fn repo_search_line(
    state: &BranchesState,
    theme: &Theme,
    width: u16,
) -> Paragraph<'static> {
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

pub(super) fn render_repo_rows(
    frame: &mut Frame,
    state: &BranchesState,
    theme: &Theme,
    area: Rect,
) {
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
            &words::abbreviate(&row.repo.repo_root, state.home()),
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
pub(super) fn counts(repo: &RepoNode) -> String {
    let worktrees = count_of(repo.worktrees.len(), "worktree", "worktrees");
    let panes: usize = repo.worktrees.iter().map(|w| w.panes.len()).sum();
    format!("{worktrees}, {}", count_of(panes, "pane", "panes"))
}

pub(super) fn render_branch_rows(
    frame: &mut Frame,
    state: &BranchesState,
    theme: &Theme,
    area: Rect,
) {
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

/// A glyph and colour for what a branch currently is, in the same visual language as the
/// agent status glyphs.
pub(super) fn branch_glyph(entry: &BranchEntry, theme: &Theme) -> (&'static str, Style) {
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

pub(super) fn branch_state_label(entry: &BranchEntry) -> String {
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
