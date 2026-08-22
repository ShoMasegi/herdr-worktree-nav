//! Drawing the pickers.
//!
//! The layout is herdr's session navigator: an accent-coloured panel holding a search line,
//! a rule, the rows, a breadcrumb for the row under the cursor, and a key hint. Rows carry
//! a gutter marking where the session currently is, connected tree glyphs, an agent status
//! glyph, a label, and a right-hand meta column. Reproduced from `src/ui/navigator.rs` in
//! herdr 0.7.4.
//!
//! Rendering is a function of state, so the whole screen is covered by snapshot tests over
//! a `TestBackend` buffer rather than by looking at a terminal.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::domain::model::RepoNode;
use crate::domain::order::Order;
use crate::domain::preview::{Preview, PreviewPane};
use crate::domain::resolve::{BranchEntry, BranchState};
use crate::domain::rows::{abbreviate, DisplayLine, Row};
use crate::port::LayoutRect;
use crate::ui::branches::{Activity, BranchesState, Step};
use crate::ui::diagram::{Fit, Frame as DiagramFrame};
use crate::ui::state::PanesState;
use crate::ui::theme::Theme;

/// Which picker is on screen. They share the panel, the search line, and the footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Panes,
    Branches,
}

/// Blank columns between the longest label and the meta column, so the two read as
/// neighbours rather than as one run of text.
const META_GAP: usize = 4;

/// The meta column never starts so far right that nothing useful fits after it.
const MIN_META_WIDTH: usize = 20;

/// Where the meta column starts: just past the longest label that has something to say,
/// so paths sit beside their rows instead of against the far edge of a wide pane.
///
/// Computed over every row rather than the visible ones, so the column does not shift as the
/// list scrolls. Rows with no meta are ignored: a long repository name has nothing to line up
/// with and should not push everyone else right.
fn meta_column(rows: &[Row], width: u16) -> usize {
    let longest = rows
        .iter()
        .filter(|row| !row.meta.is_empty())
        .map(label_end)
        .max()
        .unwrap_or(0);
    let ceiling = (width as usize).saturating_sub(MIN_META_WIDTH);
    (longest + META_GAP).min(ceiling)
}

/// How many columns a row's label region occupies: the gutter, the tree, the status glyph,
/// the label itself, and the note on a checkout with nothing running in it.
fn label_end(row: &Row) -> usize {
    // Mirrors `tree_prefix`, whose glyphs carry their own trailing space.
    let tree = if row.reference.is_group() || row.depth == 0 {
        0
    } else {
        3 * row.depth as usize + 1
    };
    // +2 for the status glyph and the space after it.
    GUTTER_WIDTH
        + tree
        + 2
        + row.label.chars().count()
        + if row.is_idle { IDLE_NOTE.len() } else { 0 }
}

/// `" ◆ "` or three spaces.
const GUTTER_WIDTH: usize = 3;
const IDLE_NOTE: &str = "  no pane";

const HELP_PANES: &[&str] = &[
    "\u{21b5} jump  n new pane  \u{21e5} branches  / search  b/w/i/d/a states  h other  r reload  esc close",
    "\u{21b5} jump  n new  \u{21e5} branches  / search  b/w/i/d/a states  esc close",
    "\u{21b5} jump  \u{21e5} branches  / search  esc close",
    "\u{21b5} jump  esc close",
];
const HELP_PANES_SEARCH: &[&str] = &[
    "\u{21b5} keep search  ctrl+u clear  esc cancel  \u{2191}\u{2193} move",
    "\u{21b5} keep  esc cancel",
];

/// The four rows the picker lays out in. Mirrors herdr's navigator geometry: search on the
/// first line, a rule under it, the body, then the breadcrumb and the key hint.
///
/// There is no panel to draw. The pickers open as popups, and herdr already frames a popup
/// with an accent-coloured border and a title — the same frame its navigator draws for
/// itself — so this fills what is inside it.
struct Panel {
    search: Rect,
    rule: Rect,
    body: Rect,
    detail: Rect,
    footer: Rect,
}

fn layout(frame: &Frame) -> Option<Panel> {
    let area = frame.area();
    if area.width < 4 || area.height < 4 {
        return None;
    }
    Some(Panel {
        search: Rect::new(area.x, area.y, area.width, 1),
        rule: Rect::new(area.x, area.y + 1, area.width, 1),
        body: Rect::new(area.x, area.y + 2, area.width, area.height - 4),
        detail: Rect::new(area.x, area.y + area.height - 2, area.width, 1),
        footer: Rect::new(area.x, area.y + area.height - 1, area.width, 1),
    })
}

pub fn draw(frame: &mut Frame, state: &PanesState, theme: &Theme, _mode: Mode) {
    let Some(panel) = layout(frame) else {
        return;
    };

    frame.render_widget(search_line(state, theme, panel.search.width), panel.search);
    render_rule(frame, panel.rule, theme);
    render_rows(frame, state, theme, panel.body);
    render_detail(frame, &state.detail(), theme, panel.detail);

    let variants = if state.is_filtering() {
        HELP_PANES_SEARCH
    } else {
        HELP_PANES
    };
    frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
}

/// `/ query` with the total on the right, or a state chip when one is active.
fn search_line(state: &PanesState, theme: &Theme, width: u16) -> Paragraph<'static> {
    let focus = if state.is_filtering() {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.dim()
    };
    let mut spans = vec![Span::styled(" / ", focus)];

    if let Some(message) = state.message() {
        spans.push(Span::styled(
            message.to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    } else if let Some(filter) = state.state_filter() {
        let (glyph, style) = theme.status_glyph(filter.status());
        spans.push(Span::styled(glyph, style.add_modifier(Modifier::BOLD)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            filter.label(),
            style.add_modifier(Modifier::BOLD),
        ));
    } else if state.query().is_empty() {
        spans.push(Span::styled("search panes", theme.dim()));
    } else {
        spans.push(Span::raw(state.query().to_string()));
    }
    if state.is_filtering() {
        spans.push(Span::styled("\u{2588}", theme.dim()));
    }

    let count = format!("{} panes", state.pane_count());
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let pad = (width as usize).saturating_sub(used + count.chars().count() + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(count, theme.dim()));
    Paragraph::new(Line::from(spans))
}

fn render_rule(frame: &mut Frame, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new("\u{2500}".repeat(area.width as usize)).style(theme.rule()),
        area,
    );
}

/// The breadcrumb, drawn over a rule so the list has a bottom edge even when it is empty.
fn render_detail(frame: &mut Frame, detail: &str, theme: &Theme, area: Rect) {
    render_rule(frame, area, theme);
    if detail.is_empty() || area.width < 4 {
        return;
    }
    let text = truncate(detail, area.width.saturating_sub(2) as usize);
    frame.render_widget(Paragraph::new(format!(" {text} ")).style(theme.dim()), area);
}

fn footer(variants: &[&'static str], theme: &Theme, width: u16) -> Paragraph<'static> {
    let text = variants
        .iter()
        .find(|text| text.chars().count() < width as usize)
        .copied()
        .unwrap_or_else(|| variants.last().copied().unwrap_or_default());
    Paragraph::new(Line::from(Span::styled(format!(" {text}"), theme.dim())))
}

fn render_rows(frame: &mut Frame, state: &PanesState, theme: &Theme, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let lines = state.lines();
    if lines.is_empty() {
        let text = if state.query().is_empty() && state.state_filter().is_none() {
            "no repositories open"
        } else {
            "nothing matches"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, theme.dim()))),
            area,
        );
        return;
    }

    let viewport = area.height as usize;
    let scroll = scroll_offset(state.cursor(), lines.len(), viewport);
    let end = lines.len().min(scroll + viewport);
    let filtering = !state.query().trim().is_empty() || state.state_filter().is_some();
    let column = meta_column(state.rows(), area.width);

    for (offset, line) in lines[scroll..end].iter().enumerate() {
        let DisplayLine::Row(index) = *line else {
            continue;
        };
        let rect = Rect::new(area.x, area.y + offset as u16, area.width, 1);
        let selected = scroll + offset == state.cursor();
        render_row(
            frame,
            &state.rows()[index],
            state.rows(),
            index,
            theme,
            rect,
            selected,
            filtering,
            column,
        );
    }
    render_scrollbar(frame, scroll, lines.len(), viewport, theme, area);
}

/// Keep the cursor on screen with as little movement as possible.
fn scroll_offset(cursor: usize, total: usize, viewport: usize) -> usize {
    if total <= viewport {
        return 0;
    }
    cursor
        .saturating_sub(viewport / 2)
        .min(total.saturating_sub(viewport))
}

#[allow(clippy::too_many_arguments)]
fn render_row(
    frame: &mut Frame,
    row: &Row,
    rows: &[Row],
    index: usize,
    theme: &Theme,
    rect: Rect,
    selected: bool,
    filtering: bool,
    meta_column: usize,
) {
    let base = if selected {
        theme.selected()
    } else {
        Style::default()
    };
    // A row kept only because an ancestor or a sibling matched: present, but receding.
    let context_only = filtering && !row.matched;

    let label_style = if selected {
        base.add_modifier(Modifier::BOLD)
    } else if context_only {
        let dim = theme.dim();
        if row.reference.is_group() {
            dim.add_modifier(Modifier::BOLD)
        } else {
            dim
        }
    } else if row.reference.is_group() {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if row.is_current {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let (glyph, glyph_style) = theme.status_glyph(row.status);
    let glyph_style = if selected {
        base.add_modifier(Modifier::BOLD)
    } else if context_only {
        theme.dim()
    } else {
        glyph_style
    };

    let gutter = if row.is_current { " \u{25c6} " } else { "   " };
    let gutter_style = if selected {
        base
    } else if row.is_current {
        Style::default().fg(theme.accent)
    } else {
        theme.dim()
    };

    let prefix = tree_prefix(rows, index);
    // Branch glyphs sit a shade behind the labels so the structure stays in the background.
    let tree_style = if selected { base } else { theme.tree() };
    let quiet = if selected { base } else { theme.dim() };

    let used = gutter.chars().count() + prefix.chars().count() + glyph.chars().count() + 1;
    let note = if row.is_idle { IDLE_NOTE.len() } else { 0 };
    // A row with nothing in the meta column may use the whole line for its label; one with
    // something has to stop short of the column so the two do not collide.
    let label_budget = if row.meta.is_empty() {
        (rect.width as usize).saturating_sub(used + note)
    } else {
        meta_column.saturating_sub(META_GAP + used + note)
    };

    let mut spans = vec![
        Span::styled(gutter, gutter_style),
        Span::styled(prefix, tree_style),
        Span::styled(glyph, glyph_style),
        Span::raw(" "),
        Span::styled(truncate(&row.label, label_budget), label_style),
    ];
    // The meta column is taken by the checkout path, so a checkout with nothing running in
    // it says so beside its name instead.
    if row.is_idle {
        spans.push(Span::styled(IDLE_NOTE, quiet));
    }

    if !row.meta.is_empty() {
        let drawn: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        // Never less than the gap, so a label that overran still gets separated from it.
        spans.push(Span::raw(
            " ".repeat(meta_column.saturating_sub(drawn).max(META_GAP)),
        ));
        let drawn: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        // One column short of the edge: the scrollbar lives there.
        let budget = (rect.width as usize).saturating_sub(drawn + 1);
        spans.push(Span::styled(middle_elide(&row.meta, budget), quiet));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(base), rect);
}

/// Tree prefix for a row, trailing space included: connected branch glyphs (`├──`, `└──`
/// for the last sibling, with `│` continuations under ancestors that still have siblings
/// below).
///
/// A group gets nothing at all. It is a heading with nothing to expand, and a caret there
/// would promise a fold this picker does not have.
fn tree_prefix(rows: &[Row], index: usize) -> String {
    let row = &rows[index];
    if row.reference.is_group() || row.depth == 0 {
        return String::new();
    }
    let mut prefix = String::new();
    for level in 1..row.depth {
        prefix.push_str(if has_following_sibling(rows, index, level) {
            "\u{2502}  "
        } else {
            "   "
        });
    }
    prefix.push_str(if has_following_sibling(rows, index, row.depth) {
        "\u{251c}\u{2500}\u{2500} "
    } else {
        "\u{2514}\u{2500}\u{2500} "
    });
    prefix
}

/// Whether another row at `depth` follows `index` before the subtree at that depth ends.
fn has_following_sibling(rows: &[Row], index: usize, depth: u8) -> bool {
    rows[index + 1..]
        .iter()
        .take_while(|row| row.depth >= depth)
        .any(|row| row.depth == depth)
}

fn render_scrollbar(
    frame: &mut Frame,
    scroll: usize,
    total: usize,
    viewport: usize,
    theme: &Theme,
    area: Rect,
) {
    if total <= viewport || area.width <= 1 {
        return;
    }
    let track = area.height as usize;
    let thumb = ((viewport * track) / total).max(1).min(track);
    let span = total.saturating_sub(viewport);
    let top = if span == 0 {
        0
    } else {
        (scroll * track.saturating_sub(thumb)) / span
    };
    for offset in 0..track {
        let filled = offset >= top && offset < top + thumb;
        let rect = Rect::new(area.x + area.width - 1, area.y + offset as u16, 1, 1);
        frame.render_widget(
            Paragraph::new(if filled { "\u{2588}" } else { "\u{2502}" }).style(if filled {
                Style::default().fg(theme.accent)
            } else {
                theme.tree()
            }),
            rect,
        );
    }
}

/// Cut out of the middle, keeping both ends. A path's head says which tree it is in and its
/// tail says which checkout, so losing the middle costs the least. Matches herdr's own
/// `middle_elide`: an even split around a single ellipsis.
pub(crate) fn middle_elide(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "\u{2026}".to_string();
    }
    let content = width - 1;
    let left = content / 2;
    let right = content - left;
    let prefix: String = text.chars().take(left).collect();
    let suffix: String = text.chars().skip(length - right).collect();
    let mut out = prefix;
    out.push('\u{2026}');
    out.push_str(&suffix);
    out
}

/// Cut to `width` characters, with an ellipsis when something was dropped.
pub(crate) fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

// ---------------------------------------------------------------------------------------
// Branches view
// ---------------------------------------------------------------------------------------

const HELP_REPO: &[&str] = &[
    "type to filter  \u{21b5} branches  \u{21e5} panes  ctrl+u clear  esc close",
    "\u{21b5} branches  \u{21e5} panes  esc close",
    "\u{21b5} branches  esc",
];
/// The branch step, when Esc has a repository list to go back to.
const HELP_BRANCH_BACK: &[&str] = &[
    "type to filter  \u{21b5} choose  ctrl+o order  ctrl+r reverse  \u{21e5} panes  ctrl+u clear  esc back",
    "\u{21b5} choose  ctrl+o order  ctrl+r reverse  \u{21e5} panes  esc back",
    "\u{21b5} choose  ctrl+o order  esc back",
    "\u{21b5} choose  esc back",
];
/// The same, with only one repository open: Esc has nowhere to go but out.
const HELP_BRANCH: &[&str] = &[
    "type to filter  \u{21b5} choose  ctrl+o order  ctrl+r reverse  \u{21e5} panes  ctrl+u clear  esc close",
    "\u{21b5} choose  ctrl+o order  ctrl+r reverse  \u{21e5} panes  esc close",
    "\u{21b5} choose  ctrl+o order  esc close",
    "\u{21b5} choose  esc",
];
const HELP_DESTINATION: &[&str] = &[
    "\u{21b5} open here  \u{2191}\u{2193} move  esc back",
    "\u{21b5} open  esc back",
];
/// While a step that can still be abandoned is running.
const HELP_WORKING_STOPPABLE: &[&str] = &["ctrl+c stop", "ctrl+c"];
/// While one that cannot: stopping now would leave a workspace nobody moved.
const HELP_WORKING: &[&str] = &["working\u{2026}"];
const HELP_FAILED: &[&str] = &["\u{21b5} close  esc close", "\u{21b5} close"];

/// One frame per draw. Braille rather than a bar because the steps have no length to
/// measure: a fetch is as long as somebody else's network.
const SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

/// Widest branch name to give a column to. One very long name must not squeeze out the
/// state and the pull request beside it.
const MAX_BRANCH_COLUMN: usize = 40;
/// Fits "checked out", the longest state word.
const STATE_COLUMN: usize = 12;

/// Widest repository name to give a column to, for the same reason as the branch column.
const MAX_REPO_COLUMN: usize = 40;
/// Fits "12 worktrees, 34 panes".
const COUNT_COLUMN: usize = 22;

pub fn draw_branches(frame: &mut Frame, state: &BranchesState, theme: &Theme) {
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
            frame.render_widget(footer(HELP_REPO, theme, panel.footer.width), panel.footer);
        }
        Step::Branch => {
            frame.render_widget(
                branch_search_line(state, theme, panel.search.width),
                panel.search,
            );
            render_rule(frame, panel.rule, theme);
            render_branch_rows(frame, state, theme, panel.body);
            render_detail(frame, &state.detail(), theme, panel.detail);
            let variants = if state.has_repo_step() {
                HELP_BRANCH_BACK
            } else {
                HELP_BRANCH
            };
            frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
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
    let mut spans = vec![Span::styled(
        " / ",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(message) = state.message() {
        spans.push(Span::styled(
            message.to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    } else if state.query().is_empty() {
        spans.push(Span::styled("search branches", theme.dim()));
    } else {
        spans.push(Span::raw(state.query().to_string()));
    }
    spans.push(Span::styled("\u{2588}", theme.dim()));
    if state.is_loading() {
        spans.push(Span::styled("  reading the remote\u{2026}", theme.dim()));
    }

    // The order sits beside the count, so a list that is not in its usual order says so
    // where the eye already goes to read how long it is. It takes the accent once it is no
    // longer the default, because that is the only way to tell without counting rows.
    let order = format!("\u{21c5} {}", state.order().label());
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

fn repo_search_line(state: &BranchesState, theme: &Theme, width: u16) -> Paragraph<'static> {
    let mut spans = vec![Span::styled(
        " / ",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(message) = state.message() {
        spans.push(Span::styled(
            message.to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    } else if state.repo_query().is_empty() {
        spans.push(Span::styled("search repositories", theme.dim()));
    } else {
        spans.push(Span::raw(state.repo_query().to_string()));
    }
    spans.push(Span::styled("\u{2588}", theme.dim()));

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

/// `1 branch`, `5 branches`. A list that says "1 branches" reads like it is guessing.
fn count_of(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

fn destination_prompt(state: &BranchesState, theme: &Theme, width: u16) -> Paragraph<'static> {
    match state.activity() {
        Activity::Choosing => {}
        // The step replaces the question, because the question has been answered.
        Activity::Working { stage, tick } => {
            return Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    SPINNER[tick % SPINNER.len()],
                    Style::default().fg(theme.accent),
                ),
                Span::raw(" "),
                Span::styled(stage.label(), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("\u{2026}", theme.dim()),
            ]));
        }
        Activity::Failed { stage, error } => {
            let head = format!(" \u{d7} {}: ", stage.label());
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
                    format!("{}: ", stage.label()),
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
    let name = state.chosen().map(|e| e.name.clone()).unwrap_or_default();
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
                pad(branch_state_label(entry), STATE_COLUMN),
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
        .map(|destination| destination.group().chars().count())
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
        let group = destination.group();
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
                    destination.label(),
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
        Preview::Blocked { caption, reason } => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(caption.clone(), theme.dim())),
                    Line::from(""),
                    Line::from(Span::styled(
                        format!("\u{26a0} {reason}"),
                        Style::default().fg(Color::Yellow),
                    )),
                ])
                .wrap(Wrap { trim: true }),
                area,
            );
        }
        Preview::Layout {
            caption,
            area: tab,
            panes,
        } => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(caption.clone(), theme.dim()))),
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

    // Then what is in each of them.
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
                    truncate(&pane.label, inner.width.saturating_sub(2) as usize),
                    name_style,
                ),
            ]
        } else {
            vec![
                Span::styled(glyph, glyph_style),
                Span::raw(" "),
                Span::styled(
                    truncate(&pane.label, inner.width.saturating_sub(2) as usize),
                    name_style,
                ),
            ]
        };

        if inner.height >= 2 && !pane.id.is_empty() {
            // Room for both: the name on top, the id under it.
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
            // One line only, so they share it.
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

fn branch_state_label(entry: &BranchEntry) -> &'static str {
    match entry.state {
        BranchState::LivePane { .. } => "running",
        BranchState::IdleWorktree { .. } => "checked out",
        BranchState::LocalRef => "local",
        BranchState::RemoteOnly => "remote",
        BranchState::New => "create",
    }
}

/// Right-pad to `width` characters so a column lines up.
fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    let mut out = text.to_string();
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(len)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;

    use crate::domain::chrome::Chrome;
    use crate::domain::dest::Destination;
    use crate::domain::model::{PaneNode, RepoNode, Tree, WorktreeNode};
    use crate::domain::progress::Stage;
    use crate::port::{AgentStatus, GitRef, PullRequest, RefKind, SplitDirection};
    use crate::ui::branches::BranchData;

    fn theme() -> Theme {
        Theme::new(Chrome::default())
    }

    fn pane(id: &str, name: Option<&str>, status: AgentStatus, focused: bool) -> PaneNode {
        let workspace = id.split(':').next().unwrap().to_string();
        PaneNode {
            pane_id: id.into(),
            tab_id: format!("{workspace}:t1"),
            workspace_id: workspace,
            display_name: name.map(str::to_string),
            agent_status: status,
            focused,
        }
    }

    fn worktree(branch: &str, primary: bool, panes: Vec<PaneNode>) -> WorktreeNode {
        WorktreeNode {
            branch: Some(branch.into()),
            checkout_path: format!("/wt/{}", branch.replace('/', "-")),
            is_primary: primary,
            open_workspace_id: panes.first().map(|p| p.workspace_id.clone()),
            panes,
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
                        worktree(
                            "main",
                            true,
                            vec![
                                pane("w1:p1", Some("claude"), AgentStatus::Working, true),
                                pane("w1:p2", None, AgentStatus::Unknown, false),
                            ],
                        ),
                        worktree(
                            "feat/login",
                            false,
                            vec![pane("w2:p1", Some("codex"), AgentStatus::Idle, false)],
                        ),
                        worktree("fix/crash", false, vec![]),
                    ],
                },
                RepoNode {
                    repo_key: "/src/site/.git".into(),
                    repo_root: "/src/site".into(),
                    display_name: "me/site".into(),
                    worktrees: vec![worktree(
                        "develop",
                        true,
                        vec![pane("w3:p1", Some("claude"), AgentStatus::Blocked, false)],
                    )],
                },
            ],
            ungrouped: vec![pane("w9:p1", None, AgentStatus::Unknown, false)],
        }
    }

    fn press(state: &mut PanesState, code: KeyCode) {
        state.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn screen(state: &PanesState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, state, &theme(), Mode::Panes))
            .unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn draws_the_tree_the_gutter_and_the_meta_column() {
        insta::assert_snapshot!(screen(&PanesState::new(tree(), None), 92, 16));
    }

    #[test]
    fn draws_a_search_with_its_non_matching_context_still_present() {
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('/'));
        for c in "codex".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        insta::assert_snapshot!(screen(&state, 92, 12));
    }

    #[test]
    fn draws_a_state_filter_as_a_chip_in_the_search_line() {
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('b'));
        insta::assert_snapshot!(screen(&state, 92, 12));
    }

    #[test]
    fn draws_panes_outside_any_repository_as_their_own_group() {
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('h'));
        insta::assert_snapshot!(screen(&state, 92, 18));
    }

    #[test]
    fn keeps_the_meta_column_readable_by_shortening_labels_in_a_narrow_pane() {
        insta::assert_snapshot!(screen(&PanesState::new(tree(), None), 46, 14));
    }

    #[test]
    fn scrolls_and_shows_a_scrollbar_when_the_list_does_not_fit() {
        let mut state = PanesState::new(tree(), None);
        for _ in 0..8 {
            press(&mut state, KeyCode::Char('j'));
        }
        insta::assert_snapshot!(screen(&state, 92, 10));
    }

    #[test]
    fn draws_nothing_matching_without_losing_the_chrome() {
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('/'));
        for c in "zzzz".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        insta::assert_snapshot!(screen(&state, 92, 10));
    }

    /// The style of the cell where `needle` starts, on the first row containing it.
    ///
    /// Column, not byte offset: the rows are full of box-drawing and status glyphs, so
    /// `str::find` would land several cells to the right of the label.
    fn style_of_row(buffer: &ratatui::buffer::Buffer, needle: &str) -> Style {
        let area = buffer.area();
        for y in area.y..area.y + area.height {
            let cells: Vec<String> = (area.x..area.x + area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect();
            let line: String = cells.concat();
            if !line.contains(needle) {
                continue;
            }
            let column = (0..cells.len())
                .find(|start| cells[*start..].concat().starts_with(needle))
                .expect("the needle starts at some cell");
            return buffer[(area.x + column as u16, y)].style();
        }
        panic!("no row containing {needle}");
    }

    fn buffer_of(state: &PanesState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, state, &theme(), Mode::Panes))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn a_row_kept_only_as_context_is_dimmed_and_a_result_is_not() {
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('/'));
        for c in "codex".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        let buffer = buffer_of(&state, 92, 12);
        // Snapshots record glyphs, not styles, so this is the only place the difference
        // between a result and the context around it is actually checked.
        assert!(
            style_of_row(&buffer, "feat/login")
                .add_modifier
                .contains(Modifier::DIM),
            "the branch is only context here"
        );
        assert!(
            !style_of_row(&buffer, "codex")
                .add_modifier
                .contains(Modifier::DIM),
            "the pane is the result"
        );
    }

    #[test]
    fn nothing_is_dimmed_as_context_when_nothing_is_being_filtered() {
        let buffer = buffer_of(&PanesState::new(tree(), None), 92, 16);
        assert!(!style_of_row(&buffer, "feat/login")
            .add_modifier
            .contains(Modifier::DIM));
    }

    #[test]
    fn the_group_rows_and_the_selection_carry_herdrs_accent() {
        // The border is herdr's — a popup is framed by the host, in this same accent — so
        // what is checked here is everything the picker itself paints with it.
        let accent = Color::Rgb(137, 180, 250);
        let theme = Theme::new(Chrome {
            accent: crate::domain::chrome::Accent::Rgb(137, 180, 250),
            ..Chrome::default()
        });
        let state = PanesState::new(tree(), None);
        let mut terminal = Terminal::new(TestBackend::new(92, 16)).unwrap();
        terminal
            .draw(|frame| draw(frame, &state, &theme, Mode::Panes))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        // No heading is ever under the cursor now, so both keep the accent foreground.
        assert_eq!(style_of_row(&buffer, "me/site").fg, Some(accent));
        assert_eq!(style_of_row(&buffer, "me/app").fg, Some(accent));
        assert_eq!(
            style_of_row(&buffer, "claude").bg,
            Some(accent),
            "the selected row — the first pane — is filled with it"
        );
        // A pane row is not a group, so it keeps the terminal's own foreground.
        assert_eq!(style_of_row(&buffer, "codex").fg, Some(Color::Reset));
    }

    #[test]
    fn a_path_too_long_for_the_column_loses_its_middle_not_its_ends() {
        // Both ends carry meaning: the head says which tree the checkout is in, the tail
        // says which checkout. Matches herdr's own middle_elide.
        assert_eq!(
            middle_elide("~/.herdr/worktrees/app/loop-review-fix-request", 26),
            "~/.herdr/wor\u{2026}w-fix-request"
        );
        assert_eq!(middle_elide("~/short", 26), "~/short");
        assert_eq!(middle_elide("~/short", 7), "~/short");
        assert_eq!(middle_elide("abcdef", 1), "\u{2026}");
        assert_eq!(middle_elide("abcdef", 0), "\u{2026}");
    }

    #[test]
    fn the_meta_column_sits_just_past_the_longest_label_that_has_one() {
        let state = PanesState::new(tree(), None);
        // `fix/crash` plus its "no pane" note is the longest row that has a path, at 27.
        assert_eq!(meta_column(state.rows(), 92), 27 + META_GAP);
    }

    #[test]
    fn a_repository_with_nothing_beside_it_does_not_push_the_column_right() {
        // Expanded, a repository row has no meta, so a long name has nothing to line up
        // with and must not move everyone else right.
        let before = meta_column(PanesState::new(tree(), None).rows(), 92);
        let mut wide = tree();
        wide.repos[0].display_name = "a-very-long-organisation/and-repository-name".into();
        let after = meta_column(PanesState::new(wide, None).rows(), 92);
        assert_eq!(before, after);
    }

    #[test]
    fn the_column_stops_short_of_the_edge_so_something_always_fits_after_it() {
        let mut tree = tree();
        tree.repos[0].worktrees[0].branch = Some("a".repeat(80));
        let state = PanesState::new(tree, None);
        assert_eq!(meta_column(state.rows(), 60), 60 - MIN_META_WIDTH);
    }

    // ---- branches view ----

    fn git_ref(name: &str, at: i64) -> GitRef {
        GitRef {
            name: name.into(),
            kind: RefKind::Local,
            committed_at: Some(at),
            subject: Some(format!("latest work on {name}")),
        }
    }

    /// The picker as it opens with two repositories in the session: on its repository
    /// step, with `me/app` — where it was summoned from — under the cursor.
    fn branches_picker() -> BranchesState {
        let repo = RepoNode {
            repo_key: "/src/app/.git".into(),
            repo_root: "/src/app".into(),
            display_name: "me/app".into(),
            worktrees: vec![
                worktree(
                    "feat/login",
                    false,
                    vec![pane("w2:p1", Some("claude"), AgentStatus::Working, false)],
                ),
                worktree("fix/crash", false, vec![]),
            ],
        };
        let other = RepoNode {
            repo_key: "/home/me/src/notes/.git".into(),
            repo_root: "/home/me/src/notes".into(),
            display_name: "me/notes".into(),
            worktrees: vec![worktree(
                "main",
                true,
                vec![pane("w3:p1", None, AgentStatus::Unknown, false)],
            )],
        };
        let snapshot: crate::port::Snapshot = serde_json::from_value(serde_json::json!({
            "version": "0.7.4",
            "protocol": 16,
            "workspaces": [
                {"workspace_id": "w1", "label": "app", "number": 1, "focused": true,
                 "active_tab_id": "w1:t1", "agent_status": "idle"},
            ],
            "tabs": [
                {"tab_id": "w1:t1", "workspace_id": "w1", "label": "agents", "number": 1,
                 "focused": true, "pane_count": 2, "agent_status": "idle"},
                {"tab_id": "w3:t1", "workspace_id": "w3", "label": "zoomed", "number": 1,
                 "focused": false, "pane_count": 1, "agent_status": "idle"},
            ],
            "panes": [
                {"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1",
                 "terminal_id": "t1", "focused": true, "agent": "claude",
                 "agent_status": "working"},
                {"pane_id": "w1:p9", "tab_id": "w1:t1", "workspace_id": "w1",
                 "terminal_id": "t9", "focused": false, "agent_status": "unknown"},
            ],
            "layouts": [
                {"tab_id": "w1:t1", "workspace_id": "w1", "zoomed": false,
                 "area": {"x": 0, "y": 0, "width": 250, "height": 79},
                 "focused_pane_id": "w1:p1",
                 "panes": [
                     {"pane_id": "w1:p1", "focused": true,
                      "rect": {"x": 0, "y": 0, "width": 250, "height": 40}},
                     {"pane_id": "w1:p9", "focused": false,
                      "rect": {"x": 0, "y": 40, "width": 250, "height": 39}}
                 ]},
                {"tab_id": "w3:t1", "workspace_id": "w3", "zoomed": true,
                 "area": {"x": 0, "y": 0, "width": 250, "height": 79},
                 "focused_pane_id": "w3:p1",
                 "panes": [{"pane_id": "w3:p1", "focused": true,
                            "rect": {"x": 0, "y": 0, "width": 250, "height": 79}}]},
            ],
        }))
        .expect("snapshot fixture should deserialize");
        BranchesState::new(
            vec![repo, other],
            Some("/src/app"),
            vec![
                Destination::SplitHere {
                    tab_id: "w1:t1".into(),
                    target_pane_id: "w1:p1".into(),
                    direction: SplitDirection::Right,
                },
                Destination::SplitHere {
                    tab_id: "w1:t1".into(),
                    target_pane_id: "w1:p1".into(),
                    direction: SplitDirection::Down,
                },
                Destination::ExistingTab {
                    tab_id: "w1:t2".into(),
                    label: "w1  app / logs".into(),
                },
                Destination::ExistingSpace {
                    workspace_id: "w3".into(),
                    label: "w3  notes \u{2192} new tab".into(),
                },
                Destination::NewSpace,
                Destination::ExistingTab {
                    tab_id: "w3:t1".into(),
                    label: "w3  notes / zoomed".into(),
                },
            ],
            snapshot,
            Some("/home/me".into()),
        )
    }

    /// The same picker, moved on into `me/app`'s branches.
    fn branches_state() -> BranchesState {
        let mut state = branches_picker();
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        state.set_data(BranchData {
            local_refs: vec![
                git_ref("feat/login", 30),
                git_ref("fix/crash", 20),
                git_ref("main", 40),
                git_ref("chore/deps", 10),
            ],
            remote_heads: vec!["feat/search".into(), "main".into()],
            pull_requests: vec![PullRequest {
                number: 123,
                title: "Add the login screen".into(),
                head_ref: "feat/login".into(),
                is_draft: true,
            }],
            loading: false,
        });
        state
    }

    fn branches_screen(state: &BranchesState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw_branches(frame, state, &theme()))
            .unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn draws_the_repositories_the_session_has_open() {
        insta::assert_snapshot!(branches_screen(&branches_picker(), 92, 12));
    }

    #[test]
    fn draws_branches_in_the_same_chrome_as_the_panes_view() {
        insta::assert_snapshot!(branches_screen(&branches_state(), 92, 12));
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
        for c in "feat/brand-new".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        insta::assert_snapshot!(branches_screen(&state, 92, 10));
    }

    #[test]
    fn draws_the_step_it_is_on_instead_of_the_question_it_already_asked() {
        let mut state = branches_state();
        for c in "chore".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
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
        for c in "chore".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
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
        for c in "chore".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        // Move to the zoomed tab, which is the last destination in this fixture.
        state.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 110, 14));
    }

    #[test]
    fn draws_the_destination_preview_at_a_realistic_size() {
        let mut state = branches_state();
        for c in "chore".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 110, 22));
    }

    #[test]
    fn draws_the_destination_step_with_split_here_selected() {
        let mut state = branches_state();
        for c in "chore".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }
}
