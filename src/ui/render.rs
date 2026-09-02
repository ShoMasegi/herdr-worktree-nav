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
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::domain::model::RepoNode;
use crate::domain::order::Order;
use crate::domain::preview::{Preview, PreviewPane};
use crate::domain::resolve::{BranchEntry, BranchState};
use crate::domain::rows::{abbreviate, marks, marks_reserve, DisplayLine, Row};
use crate::port::LayoutRect;
use crate::ui::branches::{Activity, BranchesState, Step};
use crate::ui::diagram::{Fit, Frame as DiagramFrame};
use crate::ui::state::{PanesState, Removal};
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
        + marks_reserve(row)
        + if row.is_idle { IDLE_NOTE.len() } else { 0 }
}

/// `" ◆ "` or three spaces.
const GUTTER_WIDTH: usize = 3;
const IDLE_NOTE: &str = "  no pane";
/// A checkout being removed says so where its `no pane` note would go: it is the more
/// urgent fact about the same row, and the removal is running somewhere this picker cannot
/// see. The spinner glyph follows.
const REMOVING_NOTE: &str = "  deleting ";

/// How wide the note actually drawn on a row is.
///
/// The rule behind every width here: **the meta column is a maximum over every row, so
/// nothing that can appear while the picker is up may make a row wider than it was
/// measured.** `domain::rows::marks_reserve` keeps room for the `✱` whether or not it is
/// showing, so the column does not move when a `git status` answers. The `deleting` note is
/// the other direction — it is three columns wider than the `no pane` note it replaces, and
/// it is deliberately *not* reserved, because it appears on a keypress on one row and the
/// three columns come out of that row's own label rather than out of everyone's alignment.
fn note_width(row: &Row) -> usize {
    if row.is_removing {
        // The spinner glyph follows the note.
        return REMOVING_NOTE.chars().count() + 1;
    }
    if row.is_idle {
        return IDLE_NOTE.len();
    }
    0
}

/// Widest first; the picker draws the first that fits. Each rung drops the least useful
/// thing left, so a narrow pane loses `r reload` before it loses how to move.
const HELP_PANES: &[&str] = &[
    "\u{21b5} jump  n new pane  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  shift+d remove  r reload  esc close",
    "\u{21b5} jump  n new  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  shift+d remove  esc close",
    "\u{21b5} jump  n new  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  esc close",
    "\u{21b5} jump  n new  \u{2190}\u{2192} repo  \u{21e5} branches  / search  esc close",
    // Narrow enough that something has to go: the other view outranks a way of moving
    // around this one, which the arrow keys suggest on their own.
    "\u{21b5} jump  \u{21e5} branches  / search  esc close",
    "\u{21b5} jump  esc close",
];
/// While a deletion is waiting on a yes. Says what the dialog says, in case the dialog is
/// too small a pane to have drawn everything.
const HELP_PANES_REMOVE: &[&str] = &["y delete  any other key cancels", "y delete"];
const HELP_PANES_SEARCH: &[&str] = &[
    "\u{21b5} keep search  ctrl+u clear  esc cancel  \u{2191}\u{2193} move  \u{2190}\u{2192} repo",
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
    if let Some(removal) = state.pending_removal() {
        render_removal(frame, removal, state.home(), theme, panel.body);
    }
    render_detail(frame, &state.detail(), theme, panel.detail);

    let variants = match (state.pending_removal().is_some(), state.is_filtering()) {
        (true, _) => HELP_PANES_REMOVE,
        (false, true) => HELP_PANES_SEARCH,
        (false, false) => HELP_PANES,
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
    } else {
        // A state filter and a query can both be on at once — `b` then `/` — so the chip
        // sits beside what is being typed rather than in place of it.
        if let Some(filter) = state.state_filter() {
            let (glyph, style) = theme.status_glyph(filter.status());
            spans.push(Span::styled(glyph, style.add_modifier(Modifier::BOLD)));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                filter.label(),
                style.add_modifier(Modifier::BOLD),
            ));
        }
        if !state.query().is_empty() {
            if state.state_filter().is_some() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::raw(state.query().to_string()));
        } else if !state.is_filtering() && state.state_filter().is_none() {
            // The placeholder is what to do when the field is not focused; once it is, the
            // cursor says everything and the hint is in the way of what is being typed.
            spans.push(Span::styled("search panes", theme.dim()));
        }
    }
    if state.is_filtering() {
        spans.push(Span::styled("\u{2588}", theme.dim()));
    }
    // Whether a checkout is holding uncommitted work is a walk of its whole working tree,
    // one per checkout, so the answers land after the first frame. The spinner says the
    // list is still filling in rather than finished and empty-handed — the same thing the
    // branches view does while it waits on a remote.
    if state.is_waiting() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(spinner(state.frame()), theme.dim()));
        spans.push(Span::styled(" reading working trees\u{2026}", theme.dim()));
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

/// The question a deletion asks, as a box over the list.
///
/// A dialog rather than a line in the search field: this is the one thing the picker does
/// that cannot be undone by doing it again, and it should not look like the place where
/// ordinary messages go.
fn render_removal(
    frame: &mut Frame,
    removal: &Removal,
    home: Option<&str>,
    theme: &Theme,
    body: Rect,
) {
    const TITLE: &str = "Delete this checkout?";
    const KEYS_Y: &str = "y delete";
    const KEYS_REST: &str = "     any other key cancels";

    let path = abbreviate(&removal.checkout_path, home);
    let widest = [
        TITLE.chars().count(),
        KEYS_Y.chars().count() + KEYS_REST.chars().count(),
    ]
    .into_iter()
    .chain([removal.label.chars().count() + 2, path.chars().count() + 2])
    .max()
    .unwrap_or(0);
    // Two columns of border and two of padding on each side, and a ceiling: a worktree path
    // is long enough to turn a dialog into a banner across a wide pane. What will not fit
    // loses its middle, and the breadcrumb under the list still carries the whole thing.
    const MAX_WIDTH: usize = 80;
    let width = (widest + 6).min(body.width as usize).min(MAX_WIDTH) as u16;

    let blank = Line::from("");
    let title = Line::from(Span::styled(
        TITLE,
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let branch = Line::from(Span::raw(format!("  {}", removal.label)));
    let inner_width = width.saturating_sub(6) as usize;
    let path = Line::from(Span::styled(
        format!("  {}", middle_elide(&path, inner_width)),
        theme.dim(),
    ));
    let keys = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            KEYS_Y,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(KEYS_REST, theme.dim()),
    ]);

    // Shrink by dropping the air first and the detail second, so a short pane still gets a
    // question rather than a broken box.
    let lines: Vec<Line> = match body.height {
        8.. => vec![title, blank.clone(), branch, path, blank, keys],
        6..=7 => vec![title, branch, path, keys],
        _ => vec![title, keys],
    };
    let height = (lines.len() + 2) as u16;
    if width < 8 || height > body.height {
        return;
    }

    let area = Rect::new(
        body.x + (body.width - width) / 2,
        body.y + (body.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_style(Style::default().fg(theme.accent))
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        area,
    );
}

fn render_rows(frame: &mut Frame, state: &PanesState, theme: &Theme, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let lines = state.lines();
    if lines.is_empty() {
        let text = if state.query().is_empty() && state.state_filter().is_none() {
            // Panes outside a repository are listed too, so an empty list means an empty
            // session rather than a session with nothing checked out.
            "no panes open"
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
            state.frame(),
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
    tick: usize,
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

    // What the checkout itself is: uncommitted work, and where it stands against its
    // upstream. Measured and drawn from the same string, so the two cannot drift.
    let marks = marks(row);
    let used = gutter.chars().count() + prefix.chars().count() + glyph.chars().count() + 1;
    let note = note_width(row) + marks.chars().count();
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
    // Beside the name rather than in a column, because most rows have none of it.
    if !marks.is_empty() {
        spans.push(Span::styled(marks, quiet));
    }
    // The meta column is taken by the checkout path, so a checkout with nothing running in
    // it says so beside its name instead — and one that is going says that, which is the
    // more urgent thing to know about the same row.
    if row.is_removing {
        spans.push(Span::styled(REMOVING_NOTE, quiet));
        spans.push(Span::styled(spinner(tick), quiet));
    } else if row.is_idle {
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
    // `span` is zero when the whole list fits, and there is nowhere to scroll it to then.
    let top = (scroll * track.saturating_sub(thumb))
        .checked_div(span)
        .unwrap_or(0);
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
    "\u{21b5} branches  j/k move  / search  \u{21e5} panes  q close",
    "\u{21b5} branches  / search  \u{21e5} panes  q close",
    "\u{21b5} branches  esc close",
];
const HELP_REPO_SEARCH: &[&str] = &[
    "\u{21b5} branches  ctrl+u clear  esc cancel  \u{2191}\u{2193} move",
    "\u{21b5} branches  esc cancel",
];
/// The branch step, when Esc has a repository list to go back to. Widest first, each rung
/// dropping the least useful thing left — and, as in the panes view, the other view outranks
/// a way of moving around this one, so `Tab` survives to the second-to-last rung.
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

/// Braille rather than a bar because none of the waits have a length to measure: a fetch is
/// as long as somebody else's network.
const SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

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
/// subject — stays lined up down the list, and a list with nothing gone in it reads exactly
/// as it did before `gone` existed.
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

fn spinner(frame: usize) -> &'static str {
    SPINNER[frame % SPINNER.len()]
}

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
            // The rule the other steps draw plain is a heading here. A list of branches is
            // the one thing on screen that means nothing without knowing whose branches they
            // are, and it is the same line the repository step had under its cursor.
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
        // The branch list, frozen, under a prompt asking what to call the branch being cut
        // from the row the cursor is on. The list stays because the base is on it: taking it
        // away would ask the question without showing what the answer is about.
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
    // like one that is stuck. A fetch says so louder than the listing that happens on its
    // own, because it was asked for.
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
        // The placeholder is what to do when the field is not focused; once it is, the
        // cursor says everything and the hint is in the way of what is being typed.
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

/// `1 branch`, `5 branches`. A list that says "1 branches" reads like it is guessing.
fn count_of(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
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
    // follows, and never narrower than the floor. Only a branch whose upstream is gone
    // needs more than the floor, so only the lists that have one pay for it.
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

fn branch_state_label(entry: &BranchEntry) -> String {
    let state = match entry.state {
        BranchState::LivePane { .. } => "running",
        BranchState::IdleWorktree { .. } => "checked out",
        BranchState::LocalRef => "local",
        BranchState::RemoteOnly => "remote",
        BranchState::New => "create",
    };
    // What the branch is, and then whether the remote still has what it was tracking. The
    // second is not a state of its own: a branch whose upstream is gone is still checked
    // out, or still running, and saying only `gone` would drop the half that says where it
    // is.
    match entry.upstream_gone {
        true => format!("{state} gone"),
        false => state.to_string(),
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
    use crate::port::{AgentStatus, GitRef, PullRequest, RefKind, SplitDirection, Track};
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
            track: None,
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
        // Tall enough for the whole list, including the panes that are in no repository:
        // they are a section of it like any other.
        insta::assert_snapshot!(screen(&PanesState::new(tree(), None), 92, 18));
    }

    #[test]
    fn every_checkout_says_what_state_it_is_in() {
        // The four answers, on four checkouts: ahead and behind its upstream, an upstream
        // that is gone, uncommitted work, and a checkout with nothing to report at all.
        let mut tree = tree();
        tree.repos[0].worktrees[0].track = Some(Track::Divergence {
            ahead: 2,
            behind: 1,
        });
        tree.repos[0].worktrees[1].track = Some(Track::Gone);
        tree.repos[0].worktrees[2].track = Some(Track::Divergence {
            ahead: 0,
            behind: 3,
        });
        let mut state = PanesState::new(tree, None);
        state.set_dirty(vec!["/wt/feat-login".into(), "/wt/fix-crash".into()]);
        insta::assert_snapshot!(screen(&state, 92, 18));
    }

    #[test]
    fn nothing_is_claimed_about_a_working_tree_that_has_not_been_read_yet() {
        // Asking costs a process per checkout, so the answers land after the first frame.
        // Until one does, the row says nothing about uncommitted work — and the prompt line
        // says the list is still filling in rather than finished and empty-handed.
        let mut state = PanesState::new(tree(), None);
        state.set_waiting(true);
        insta::assert_snapshot!(screen(&state, 92, 18));
    }

    #[test]
    fn a_checkout_being_removed_says_so_where_its_no_pane_note_was() {
        // The removal is running in another process and may well outlive this window, so
        // the row has to say what is happening to it rather than simply going quiet. The
        // cursor has stepped off it: there is nothing left to do to it from here.
        let mut state = PanesState::new(tree(), None);
        state.set_removing(vec!["/wt/fix-crash".into()]);
        insta::assert_snapshot!(screen(&state, 92, 18));
    }

    #[test]
    fn an_empty_search_field_with_the_keyboard_shows_only_its_cursor() {
        // The placeholder is advice about a field you are not in. Leaving it under the
        // cursor would read as text that will not go away.
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('/'));
        insta::assert_snapshot!(screen(&state, 92, 6));
    }

    #[test]
    fn a_state_chip_and_a_typed_query_sit_beside_each_other() {
        // Both can be on at once, and the chip used to be drawn in place of the query —
        // so the letters went in and nothing appeared.
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('b'));
        press(&mut state, KeyCode::Char('/'));
        for c in "cla".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        insta::assert_snapshot!(screen(&state, 92, 6));
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
    fn draws_the_question_a_deletion_asks_before_it_happens() {
        let mut state = PanesState::new(tree(), None);
        // Down to `fix/crash`, the checkout with nothing running in it.
        for _ in 0..3 {
            press(&mut state, KeyCode::Char('j'));
        }
        press(&mut state, KeyCode::Char('D'));
        insta::assert_snapshot!(screen(&state, 92, 12));
    }

    #[test]
    fn the_question_shrinks_rather_than_breaking_in_a_short_pane() {
        // The air goes first, then the detail. A pane too short for a box at all gets the
        // key hint, which says the same thing.
        let mut state = PanesState::new(tree(), None);
        for _ in 0..3 {
            press(&mut state, KeyCode::Char('j'));
        }
        press(&mut state, KeyCode::Char('D'));
        insta::assert_snapshot!(screen(&state, 92, 10));
    }

    #[test]
    fn draws_a_state_filter_as_a_chip_in_the_search_line() {
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('b'));
        insta::assert_snapshot!(screen(&state, 92, 12));
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
        // `fix/crash`, its "no pane" note, and the three columns kept for a `✱` that has
        // not arrived yet: 30. Nothing else in this tree has anything to line up with.
        assert_eq!(meta_column(state.rows(), 92), 30 + META_GAP);
    }

    #[test]
    fn starting_a_removal_does_not_move_the_meta_column() {
        // The wider note is paid for out of that row's own label, not out of everyone
        // else's alignment: a removal starts on a keypress, and a list that shifts
        // sideways under one is a list nobody can read while tidying up.
        let mut state = PanesState::new(tree(), None);
        let before = meta_column(state.rows(), 92);
        state.set_removing(vec!["/wt/fix-crash".into()]);
        assert_eq!(meta_column(state.rows(), 92), before);
    }

    #[test]
    fn an_answer_about_uncommitted_work_does_not_move_the_meta_column_either() {
        // The same rule, and the case it was written for: these answers arrive a beat after
        // the first frame, with the list already on screen and being read.
        let mut state = PanesState::new(tree(), None);
        let before = meta_column(state.rows(), 92);
        state.set_dirty(vec!["/wt/fix-crash".into(), "/wt/main".into()]);
        assert_eq!(meta_column(state.rows(), 92), before);
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
            track: None,
            worktree_path: None,
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
    fn branch_data() -> BranchData {
        BranchData {
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
            fetching: false,
        }
    }

    fn branches_state() -> BranchesState {
        let mut state = branches_picker();
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        state.set_data(branch_data());
        state
    }

    /// Open the search box and type into it: letters are commands until `/` is pressed.
    fn search(state: &mut BranchesState, text: &str) {
        state.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in text.chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
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
    fn a_branch_whose_upstream_is_gone_says_so_beside_what_it_is() {
        // The ordinary end of a merged branch: GitHub deleted the head, a pruning fetch
        // noticed, and the local branch and its checkout are all that is left. `gone` goes
        // inside the state column, which widens for this list and no other, so the subjects
        // beside it stay lined up.
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

    /// The rule over the list carries the repository the branches belong to, and the
    /// breadcrumb under it no longer repeats it. Entered from the second repository rather
    /// than the first, so a heading wired to whichever one the picker opened on would show.
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

    /// The prompt names what the branch is being cut from, and the list stays put so the
    /// row it names is still on screen underneath it.
    #[test]
    fn draws_the_prompt_for_a_branch_started_from_the_one_under_the_cursor() {
        let mut state = branches_state();
        state.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        for c in "feat/login-v2".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    /// A name git would take is not the same as a name this repository has room for, and the
    /// reason goes where the name is rather than under the list.
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
