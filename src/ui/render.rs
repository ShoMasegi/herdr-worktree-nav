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

use crate::domain::model::{PaneNode, RepoNode};
use crate::domain::order::Order;
use crate::domain::preview::{Preview, PreviewPane};
use crate::domain::removal::Removal;
use crate::domain::resolve::{BranchEntry, BranchState};
use crate::domain::rows::{self, abbreviate, marks, marks_reserve, DisplayLine, Row, UNNAMED_PANE};
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

/// The fewest columns a path keeps before the note beside the name is dropped instead.
///
/// Enough for `/…re-deps` — an ellipsis and a tail, and the tail is what tells checkouts
/// apart. A note is allowed past the meta column (see `MIN_LABEL`), but not this far: a row
/// that had given the name away for its reason then gave the path away too, and at 31
/// columns was a `[x]`, `PR #1234 merged`, and nothing either was about.
const MIN_PATH: usize = 8;

/// The fewest columns a label keeps before the note beside it is dropped instead.
///
/// Enough for `fea…` — the start of a name and the ellipsis that says it is one. In a narrow
/// pane a `gone` marker and a `no pane` note together could take the whole budget, and the
/// row then carried a marker, a note and no way of telling which checkout either was about.
///
/// What this can take away is worth stating exactly. It can drop `no pane`, which is the one
/// note that is only ever a remark. It cannot drop `gone`, which is a marker rather than a
/// note and is measured into the meta column; it cannot drop `deleting`; and it cannot drop
/// what a sweep says beside a box — `PR #123 merged`, `PR unknown` — because ADR 0011 asks
/// for exactly that: a mark whose reason is invisible is one the user trusts blindly. Where
/// those do not fit beside the name, the name gives way instead, down to nothing, and the
/// path in the meta column is what still says which checkout the row is about — which is
/// why the path is the one thing a note is never allowed to take: past `MIN_PATH`, the note
/// gives way after all. The call site says why each of the two is not negotiable.
const MIN_LABEL: usize = 4;

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
/// the label itself, the room kept for what the checkout says about itself, and the note on
/// one with nothing running in it.
///
/// The rule behind which of those are counted here: **the meta column is a maximum over
/// every row, so nothing that can appear while the picker is up may make a row wider than it
/// was measured.** `domain::rows::marks_reserve` therefore keeps room for the `✱` whether or
/// not it is showing — `✱` and `?` are the same width, so one reserve serves both. The
/// `deleting` note is the deliberate exception: it is wider than the `no pane` note it
/// replaces — by three columns on the idle row it is normally drawn on — and is left out,
/// because it appears on a keypress on one row and those columns come out of that row's own
/// label rather than out of everyone else's alignment.
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

/// What a row says about itself in a sweep — `PR #123 merged` or `PR unknown` — with the gap
/// the other notes use.
///
/// Not `gone`, which the row already carries as its upstream marker, and not a refusal,
/// which is said by the absence of a box and answered on the prompt line to whoever presses
/// `Space`. [`domain::sweep::Mark::note`](crate::domain::sweep::Mark::note) is where both of
/// those are decided and why.
///
/// Left out of `label_end` for the reason `REMOVING_NOTE` is, and it is the same reason
/// pointed at a different key: this appears when the user presses `Shift-S` and changes
/// again when `gh` answers, and measuring it would move every path in the list sideways
/// twice — including the paths of repositories nothing has been said about. So the columns
/// come out of the label of the row that wanted them.
fn sweep_note(row: &Row) -> Option<String> {
    Some(format!("  {}", row.sweep.as_ref()?.note()?))
}

/// How wide the note actually drawn on a row is — as opposed to how wide `label_end`
/// measured it, which is where the reasoning about the two lives.
fn note_width(row: &Row) -> usize {
    if row.is_removing {
        // The spinner glyph follows the note.
        return REMOVING_NOTE.chars().count() + 1;
    }
    if let Some(note) = sweep_note(row) {
        return note.chars().count();
    }
    if row.is_idle {
        return IDLE_NOTE.len();
    }
    0
}

/// The three columns a checkout's mark takes in the gutter during a sweep, or `None` for a
/// row that is not a checkout.
///
/// Exactly the width of the `" ◆ "` it replaces, so nothing else on the line moves when the
/// sweep opens. What it would replace is where the session currently is — the less useful of
/// the two while the question on screen is what to delete, and said by the panes listed
/// under the checkout anyway.
///
/// As it stands the two never meet: `domain::rows::flatten` gives every worktree row
/// `is_current: false`, so a row with a box is never a row with a diamond, and the order the
/// gutter takes them in is unobservable — a mutation that swaps them survives. Whoever makes
/// a checkout able to be current has to decide it, which is why the gutter matches on the
/// pair rather than checking one and falling through to the other.
fn sweep_box(row: &Row) -> Option<&'static str> {
    let mark = row.sweep.as_ref()?;
    Some(if mark.is_going() {
        "[x]"
    } else if mark.is_markable() {
        "[ ]"
    } else {
        // No box at all rather than an empty one: an empty box is an invitation, and `Space`
        // does nothing here.
        "   "
    })
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
/// While a sweep is on. `\u{21b5}` is not offered: it removes what is marked, and that is
/// not wired up yet.
const HELP_PANES_SWEEP: &[&str] = &[
    "space mark  \u{2191}\u{2193} move  \u{2190}\u{2192} repo  shift+s done  esc done",
    "space mark  \u{2191}\u{2193} move  esc done",
    "space mark  esc done",
];
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

/// `false` when a removal is waiting on an answer and this pane was too small to put the
/// question in. The loop takes the question back when that happens — see `render_removal`.
pub fn draw(frame: &mut Frame, state: &PanesState, theme: &Theme, _mode: Mode) -> bool {
    let Some(panel) = layout(frame) else {
        return state.pending_removal().is_none();
    };

    frame.render_widget(search_line(state, theme, panel.search.width), panel.search);
    render_rule(frame, panel.rule, theme);
    render_rows(frame, state, theme, panel.body);
    let asked = match state.pending_removal() {
        Some(removal) => render_removal(frame, removal, state.home(), theme, panel.body),
        None => true,
    };
    render_detail(frame, &state.detail(), theme, panel.detail);

    let variants = match (
        state.pending_removal().is_some(),
        state.is_sweeping(),
        state.is_filtering(),
    ) {
        (true, _, _) => HELP_PANES_REMOVE,
        // Before the search variant, because `/` does nothing during a sweep — the keys a
        // footer offers have to be the keys that answer.
        (false, true, _) => HELP_PANES_SWEEP,
        (false, false, true) => HELP_PANES_SEARCH,
        (false, false, false) => HELP_PANES,
    };
    frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
    asked
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
        } else if state.is_sweeping() {
            // `/` does nothing during a sweep, so the field says what the mode is instead of
            // offering a search that would not run. Where `gh` failed, it says that instead:
            // it is the one thing the rows cannot say for themselves — they can say a
            // repository could not be judged, not why.
            match state.sweep_trouble() {
                Some(trouble) => spans.push(Span::styled(trouble.to_string(), theme.dim())),
                None => spans.push(Span::styled("sweep", theme.dim())),
            }
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
    //
    // A checkout git would not answer for says so on its own row rather than here — see
    // `domain::rows::marks`, and `docs/adr/0011-what-may-be-swept.md`, which puts the
    // unknown on the row it belongs to for the same reason.
    if state.is_waiting() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(spinner(state.frame()), theme.dim()));
        spans.push(Span::styled(" reading working trees\u{2026}", theme.dim()));
    }
    // Its own spinner, because until `gh` answers the rows are showing what git alone
    // decided — a smaller sweep than the one the user is about to get, and one that is about
    // to change under their cursor.
    if state.is_asking_gh() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(spinner(state.frame()), theme.dim()));
        spans.push(Span::styled(" asking gh\u{2026}", theme.dim()));
    }

    // During a sweep the number being decided about is how many are going, not how many
    // panes are open.
    let count = if state.is_sweeping() {
        format!("{} marked", state.marked_count())
    } else {
        format!("{} panes", state.pane_count())
    };
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
/// `false` when the question could not be drawn at all. The caller must then take the
/// question back: a picker that leaves `y` armed over a box nobody saw is asking a question
/// it never put on screen, and the key hint at the bottom is not that question — it says
/// which keys answer, never what is being answered.
fn render_removal(
    frame: &mut Frame,
    removal: &Removal,
    home: Option<&str>,
    theme: &Theme,
    body: Rect,
) -> bool {
    const TITLE: &str = "Delete this checkout?";
    const CLOSING: &str = "  these panes close:";

    const KEYS_Y: &str = "y delete";
    const KEYS_REST: &str = "     any other key cancels";

    let path = abbreviate(removal.checkout_path(), home);
    // Uncommitted work is git's to protect and it does. What a working agent has in flight
    // has no other safety net, so the question names every pane that stops, in the words the
    // list behind the box uses for the same panes.
    let name_column = removal
        .panes()
        .iter()
        .map(|pane| {
            pane.display_name
                .as_deref()
                .unwrap_or(UNNAMED_PANE)
                .chars()
                .count()
        })
        .max()
        .unwrap_or(0);
    let state_column = removal
        .panes()
        .iter()
        .map(|pane| agent_state(pane).chars().count())
        .max()
        .unwrap_or(0);
    let closing: Vec<String> = removal
        .panes()
        .iter()
        .map(|pane| {
            format!(
                " {}  {}   {}",
                pad(
                    pane.display_name.as_deref().unwrap_or(UNNAMED_PANE),
                    name_column
                ),
                pad(agent_state(pane), state_column),
                pane.pane_id
            )
        })
        .collect();
    // The question that carries the count is the one the smallest box uses, so it has to be
    // measured even when the list is what ends up being drawn.
    let counted = match removal.panes().len() {
        0 => TITLE.to_string(),
        1 => "Delete this checkout and close 1 pane?".to_string(),
        many => format!("Delete this checkout and close {many} panes?"),
    };
    let widest = [
        TITLE.chars().count(),
        counted.chars().count(),
        KEYS_Y.chars().count() + KEYS_REST.chars().count(),
    ]
    .into_iter()
    .chain([
        removal.label().chars().count() + 2,
        path.chars().count() + 2,
    ])
    .chain(closing.iter().map(|line| line.chars().count() + 3))
    .chain((!closing.is_empty()).then(|| CLOSING.chars().count()))
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
    let branch = Line::from(Span::raw(format!("  {}", removal.label())));
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

    // The panes, each with the glyph its row carries, so the one that is working is as
    // obvious here as it is in the list behind the box.
    let mut panes: Vec<Line> = Vec::new();
    if !removal.panes().is_empty() {
        panes.push(Line::from(Span::styled(CLOSING, theme.dim())));
        for (pane, text) in removal.panes().iter().zip(&closing) {
            let (glyph, glyph_style) = theme.status_glyph(pane.agent_status);
            panes.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(glyph, glyph_style),
                // One column narrower than the path's budget, which is what the glyph
                // costs. (`inner_width` is already two narrower than the text area.)
                Span::styled(
                    middle_elide(text, inner_width.saturating_sub(1)),
                    theme.dim(),
                ),
            ]));
        }
    }

    // Shrink by dropping the air first and the detail second, so a short pane still gets a
    // question rather than a broken box. What is about to stop outlasts everything but the
    // question itself: a branch and a path can be read from the breadcrumb behind the box,
    // and the panes cannot be read anywhere. Their names outlast the path by one rung and
    // go with the branch on the next, and when even a line each will not fit, the question
    // itself takes over
    // their number — so the smallest box a checkout with panes can have is exactly as small
    // as one without, and there is no height at which `y` is armed over a box that never
    // said panes would close.
    let counted_line = Line::from(Span::styled(
        counted.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let spaced = match panes.is_empty() {
        true => vec![blank.clone()],
        false => [vec![blank.clone()], panes.clone(), vec![blank.clone()]].concat(),
    };
    let candidates = [
        [
            vec![title.clone(), blank.clone(), branch.clone(), path.clone()],
            spaced,
            vec![keys.clone()],
        ]
        .concat(),
        [
            vec![title.clone(), branch.clone(), path],
            panes.clone(),
            vec![keys.clone()],
        ]
        .concat(),
        [vec![title, branch], panes, vec![keys.clone()]].concat(),
        vec![counted_line, keys],
    ];
    // A box too narrow for the question is as bad as no box: `Delete this checkout and close
    // 2 panes?` clipped to `Delete this checkout` is a complete sentence and a false one.
    // The caller takes a `false` from here as "this cannot be asked", and cancels.
    if width < 8 || (width as usize) < counted.chars().count() + 6 {
        return false;
    }
    let Some(lines) = candidates
        .into_iter()
        .find(|lines| lines.len() + 2 <= body.height as usize)
    else {
        return false;
    };
    let height = (lines.len() + 2) as u16;

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
    true
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

    let sweep_box = sweep_box(row);
    let gutter = match (sweep_box, row.is_current) {
        (Some(marked), _) => marked,
        (None, true) => " \u{25c6} ",
        (None, false) => "   ",
    };
    let gutter_style = if selected {
        base
    } else if sweep_box.is_some_and(|marked| marked.starts_with('[')) || row.is_current {
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
    // A row with nothing in the meta column may use the whole line for its label; one with
    // something has to stop short of the column so the two do not collide.
    let room = if row.meta.is_empty() {
        (rect.width as usize).saturating_sub(used + marks.chars().count())
    } else {
        meta_column.saturating_sub(META_GAP + used + marks.chars().count())
    };
    // What the note would leave, and whether it may take that much.
    //
    // A note gives way whole rather than truncated: half of `PR #123 merged` says nothing,
    // and the number is the checkable part.
    //
    // Two notes do not give way. `no pane` is a remark about a checkout the user may still
    // act on, and the name is worth more than the remark. `deleting` is not a remark: a row
    // being removed has stopped being about its checkout and started being about an
    // operation — it is the one row the cursor will not stop on to explain itself, it cannot
    // be marked or removed again, and the note is the whole of what the picker adds over the
    // toast (`docs/adr/0014-removing-outlives-the-picker.md`). Dropping it drew a removal in
    // flight as a perfectly ordinary row. And a sweep's note is the reason for the box in
    // the gutter, which ADR 0011 says may not be shown without one; dropped, a `[x]` sat on
    // a row with nothing to say why, on the list `Enter` will act on. The name gives way
    // instead, down to nothing: the path still says which checkout it is.
    let with_note = room.saturating_sub(note_width(row));
    // But never past the path. A note wider than the room is drawn on into the meta column,
    // and the path there is the last thing on the row that says which checkout it is — so a
    // note is kept only while the path keeps `MIN_PATH` columns after it. Narrower than
    // that, the name gets the room back and the row is a name and a path, which is what it
    // was before there were notes: a `[x]` with a reason and nothing the reason is about
    // is worse than a `[x]` with a name and no reason.
    let path_after_note = if row.meta.is_empty() {
        usize::MAX
    } else {
        let drawn = used + marks.chars().count() + with_note + note_width(row);
        let gap = meta_column.saturating_sub(drawn).max(META_GAP);
        (rect.width as usize).saturating_sub(drawn + gap + 1)
    };
    let fits = path_after_note >= MIN_PATH.min(row.meta.chars().count());
    let not_negotiable = row.is_removing || sweep_note(row).is_some();
    let keeps_its_note = fits && (not_negotiable || with_note >= MIN_LABEL);
    let label_budget = if keeps_its_note { with_note } else { room };

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
    if !keeps_its_note {
        // The name won the columns. Nothing goes here.
    } else if row.is_removing {
        spans.push(Span::styled(REMOVING_NOTE, quiet));
        spans.push(Span::styled(spinner(tick), quiet));
    } else if let Some(note) = sweep_note(row) {
        // Why this row is going, or why it cannot. A mark whose reason is invisible is one
        // the user either trusts blindly or clears wholesale —
        // `docs/adr/0011-what-may-be-swept.md`.
        spans.push(Span::styled(note, quiet));
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
    // What the branch is, and then whether git can still find what it tracks. The second is
    // not a state of its own: a branch whose upstream is gone is still checked out, or still
    // running, and saying only `gone` would drop the half that says where it is.
    match entry.upstream_gone() {
        true => format!("{state} gone"),
        false => state.to_string(),
    }
}

/// What an agent is doing, in the words the list behind the box uses. Empty for a pane with
/// no agent: the glyph beside it already says there is nothing to report, and a column of
/// `unknown` would be noise.
fn agent_state(pane: &PaneNode) -> &'static str {
    rows::status_label(pane.agent_status).unwrap_or("")
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
    use crate::domain::model::WorkingTree;
    use std::collections::BTreeMap;

    /// The answers map, spelled out per checkout. These tests care which of the four shapes
    /// a checkout is in, which is the thing the map made sayable.
    fn answers(pairs: &[(&str, WorkingTree)]) -> BTreeMap<String, WorkingTree> {
        pairs
            .iter()
            .map(|(path, answer)| ((*path).to_string(), *answer))
            .collect()
    }
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use std::num::NonZeroU32;

    use crate::domain::chrome::Chrome;
    use crate::domain::dest::Destination;
    use crate::domain::model::{PaneNode, RepoNode, Tree, WorktreeNode};
    use crate::domain::progress::Stage;
    use crate::domain::sweep::RepoRoot;
    use crate::port::{AgentStatus, GitRef, PullRequest, RefKind, SplitDirection, Track};
    use crate::port::{PullRequestOutcome, SettledPullRequest, SettledPullRequests};
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
            .draw(|frame| {
                draw(frame, state, &theme(), Mode::Panes);
            })
            .unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn draws_the_tree_the_gutter_and_the_meta_column() {
        // Tall enough for the whole list, including the panes that are in no repository:
        // they are a section of it like any other.
        insta::assert_snapshot!(screen(&PanesState::new(tree(), None), 92, 18));
    }

    /// The panes tree with a sweep on, one checkout in each shape it can be in: going,
    /// staying, refused for a pane, refused for being the repository itself.
    fn swept() -> PanesState {
        let mut tree = tree();
        // Finished with: gone upstream, nothing running in it.
        tree.repos[0].worktrees[2].track = Some(Track::Gone);
        // Gone too, but somebody is working in it.
        tree.repos[0].worktrees[1].track = Some(Track::Gone);
        // Nobody is finished with this one, and the user may still say otherwise.
        tree.repos[1]
            .worktrees
            .push(worktree("chore/deps", false, vec![]));
        let mut state = PanesState::new(tree, None);
        state.set_working_trees(answers(&[
            ("/wt/main", WorkingTree::Clean),
            ("/wt/feat-login", WorkingTree::Clean),
            ("/wt/fix-crash", WorkingTree::Clean),
            ("/wt/develop", WorkingTree::Clean),
            ("/wt/chore-deps", WorkingTree::Clean),
        ]));
        press(&mut state, KeyCode::Char('S'));
        state
    }

    #[test]
    fn a_sweep_puts_a_box_in_the_gutter_and_the_reason_beside_the_name() {
        // Every row says what is happening to it: `fix/crash` goes because its upstream is
        // gone, `feat/login` cannot because somebody is working in it, the two primaries
        // cannot because git will not take them, and the count on the right is how many are
        // going rather than how many panes are open.
        insta::assert_snapshot!(screen(&swept(), 92, 20));
    }

    /// The style of one cell. `screen()` serialises characters and throws every style away,
    /// so a snapshot can show `[x]` in the right column and say nothing at all about whether
    /// it is drawn as a mark or as chrome.
    fn cell_style(state: &PanesState, width: u16, height: u16, x: u16, y: u16) -> Style {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, state, &theme(), Mode::Panes);
            })
            .unwrap();
        terminal.backend().buffer()[(x, y)].style()
    }

    /// Which row a label is drawn on, counting the two lines above the list.
    fn label_under_cursor(state: &PanesState) -> String {
        match state.lines()[state.cursor()] {
            crate::domain::rows::DisplayLine::Row(index) => state.rows()[index].label.clone(),
            crate::domain::rows::DisplayLine::Spacer => String::new(),
        }
    }

    fn line_of(state: &PanesState, label: &str) -> u16 {
        let index = state
            .rows()
            .iter()
            .position(|row| row.label == label)
            .unwrap_or_else(|| panic!("no row labelled {label}"));
        let at = state
            .lines()
            .iter()
            .position(|line| *line == DisplayLine::Row(index))
            .expect("the row is drawn");
        at as u16 + 2
    }

    #[test]
    fn a_mark_is_drawn_as_a_mark_and_a_refusal_is_not_drawn_as_one() {
        // The gutter is three columns that mean four different things — a box that is
        // ticked, a box that is empty, no box at all, and the diamond where the session is.
        // Every snapshot in this file would draw all four identically if the colour were
        // wrong, because none of them record a colour.
        let state = swept();
        let accent = Some(theme().accent);

        let marked = line_of(&state, "fix/crash");
        assert_eq!(
            cell_style(&state, 92, 20, 1, marked).fg,
            accent,
            "a checkout that is going says so in the colour the picker uses for a mark"
        );

        let markable = line_of(&state, "chore/deps");
        assert_eq!(
            cell_style(&state, 92, 20, 1, markable).fg,
            accent,
            "and so does an empty box, which is an invitation to press Space"
        );

        // Not `main`: the cursor opens on it, and a selected row is drawn in the selection's
        // colours whatever its gutter says.
        let refused = cell_style(&state, 92, 20, 1, line_of(&state, "develop"));
        assert_ne!(
            refused.fg, accent,
            "a refusal has no box, and three accented spaces would be a mark nobody made"
        );
        assert!(
            refused.add_modifier.contains(Modifier::DIM),
            "it is chrome, drawn the way the gutter is drawn on every other quiet row"
        );

        // And the mark's accent is not taken from the diamond that says where the session
        // is: the two share the three columns and share the colour, and outside a sweep the
        // diamond is the only thing that wears it.
        let mut ordinary = PanesState::new(tree(), None);
        // Off the focused pane, since a selected row is drawn in the selection's colours
        // whatever else it is.
        press(&mut ordinary, KeyCode::Char('j'));
        assert_eq!(
            cell_style(&ordinary, 92, 18, 1, line_of(&ordinary, "claude")).fg,
            accent,
            "the row the session is on says so, sweep or no sweep"
        );
    }

    #[test]
    fn a_removal_in_flight_says_so_at_every_width_the_picker_supports() {
        // The rule that keeps a row's name in a narrow pane took this away for every width
        // from 37 to 48: a removal running somewhere the picker cannot see was drawn as a
        // perfectly ordinary row — no note, no spinner — while the cursor silently would not
        // stop on it and `Shift-D` could not reach it. It is the whole of what the picker
        // adds over the toast, and the row it is on has nothing left to decide about.
        let mut state = PanesState::new(tree(), None);
        state.set_removing(vec!["/wt/fix-crash".into()]);
        for width in [46u16, 53, 60, 92] {
            let drawn = screen(&state, width, 16);
            let row = drawn
                .lines()
                .find(|line| line.contains("/wt/fix-crash"))
                .unwrap_or_else(|| panic!("no fix/crash row at {width}"));
            assert!(
                row.contains("deleting"),
                "at {width} columns the row says nothing about the removal: {row}"
            );
        }
    }

    #[test]
    fn a_narrow_pane_still_says_which_checkout_a_sweep_is_talking_about() {
        // The width the picker already supports. A sweep's reason is drawn without being
        // measured, so it comes out of this row's own label — and at this width there is
        // not a name's worth and a reason's worth both. The reason wins: the box beside it
        // is a suggestion to delete, and ADR 0011 does not allow one without its reason
        // showing. The path in the meta column is what still says which checkout the row
        // is about, and it is the last thing on the row to be given away — never below
        // `MIN_PATH` columns, at which point the reason gives way instead.
        let mut state = swept();
        let answered = SettledPullRequests::All(vec![SettledPullRequest {
            number: 1234,
            head_ref: "chore/deps".to_string(),
            from_a_fork: false,
            outcome: PullRequestOutcome::Merged,
        }]);
        let asked: BTreeMap<_, _> = state
            .tree()
            .repos
            .iter()
            .map(|repo| (RepoRoot::of(repo), Some(answered.clone())))
            .collect();
        state.set_settled(asked, None, false);
        insta::assert_snapshot!(screen(&state, 46, 16));
    }

    #[test]
    fn a_mark_keeps_its_reason_at_every_width_the_picker_supports() {
        // The snapshot above pins one width. This pins the rule: whatever else the row
        // gives up, the reason for its box is not it — from the narrowest pane the suite
        // calls supported to one wide enough that nothing has to give.
        let mut state = swept();
        let answered = SettledPullRequests::All(vec![SettledPullRequest {
            number: 1234,
            head_ref: "chore/deps".to_string(),
            from_a_fork: false,
            outcome: PullRequestOutcome::Merged,
        }]);
        let asked: BTreeMap<_, _> = state
            .tree()
            .repos
            .iter()
            .map(|repo| (RepoRoot::of(repo), Some(answered.clone())))
            .collect();
        state.set_settled(asked, None, false);
        for width in [46u16, 53, 60, 92] {
            let drawn = screen(&state, width, 16);
            let row = drawn
                .lines()
                .find(|line| line.contains("/wt/chore-deps"))
                .unwrap_or_else(|| panic!("no chore/deps row at {width}"));
            assert!(row.contains("[x]"), "marked at {width}: {row}");
            assert!(
                row.contains("PR #1234 merged"),
                "and says why at {width}: {row}"
            );
        }
    }

    #[test]
    fn a_note_never_pushes_the_path_off_the_row() {
        // Measured rather than reasoned about, at every width from 24 to 92: the marked row
        // and the row being removed keep the tail of their path throughout, and each note
        // appears at the width its path first keeps `MIN_PATH` columns beside it — 39 for
        // `PR #1234 merged`, 34 for `deleting` — and at every width above. Before this rule
        // the marked row at 28 to 31 columns was a box, a reason, and no path at all, and
        // from 32 to 44 a path elided down to as little as `…`. (Narrower than 28 the meta
        // column is short of the path on every row, note or no note; that is `MIN_META_WIDTH`.)
        let mut marked = swept();
        let answered = SettledPullRequests::All(vec![SettledPullRequest {
            number: 1234,
            head_ref: "chore/deps".to_string(),
            from_a_fork: false,
            outcome: PullRequestOutcome::Merged,
        }]);
        let asked: BTreeMap<_, _> = marked
            .tree()
            .repos
            .iter()
            .map(|repo| (RepoRoot::of(repo), Some(answered.clone())))
            .collect();
        marked.set_settled(asked, None, false);
        let mut removing = PanesState::new(tree(), None);
        removing.set_removing(vec!["/wt/fix-crash".into()]);

        for width in 24u16..=92 {
            let drawn = screen(&marked, width, 16);
            let row = drawn
                .lines()
                .find(|line| line.contains("[x]") && line.contains("deps"))
                .unwrap_or_else(|| panic!("the marked row lost its path at {width}"));
            assert_eq!(
                row.contains("PR #1234 merged"),
                width >= 39,
                "the reason, at {width}: {row}"
            );
            // Whole from 45, where the note leaves it the room, and from 28 to 38, where
            // there is no note. Narrower than 28 the meta column itself is short of the
            // path, note or no note.
            assert_eq!(
                row.contains("/wt/chore-deps"),
                width >= 45 || (28..=38).contains(&width),
                "the whole path, at {width}: {row}"
            );

            let drawn = screen(&removing, width, 16);
            let row = drawn
                .lines()
                .find(|line| line.contains("rash"))
                .unwrap_or_else(|| panic!("the row being removed lost its path at {width}"));
            assert_eq!(
                row.contains("deleting"),
                width >= 34,
                "the note, at {width}: {row}"
            );
        }
    }

    #[test]
    fn a_path_shorter_than_min_path_asks_only_for_its_own_length() {
        // `MIN_PATH` is a floor for paths that have that much to show. A four-character
        // path is whole in four columns, and holding its note back until eight were free
        // would have dropped the reason at 35 to 38 columns for nothing. Measured: with
        // `/w/x` the note appears from 35; with `/wt/chore-deps` it appears from 39.
        let mut state = swept();
        let mut tree = state.tree().clone();
        tree.repos[1].worktrees[1].checkout_path = "/w/x".to_string();
        state.replace_tree(tree);
        state.set_working_trees(answers(&[("/w/x", WorkingTree::Clean)]));
        let answered = SettledPullRequests::All(vec![SettledPullRequest {
            number: 1234,
            head_ref: "chore/deps".to_string(),
            from_a_fork: false,
            outcome: PullRequestOutcome::Merged,
        }]);
        let asked: BTreeMap<_, _> = state
            .tree()
            .repos
            .iter()
            .map(|repo| (RepoRoot::of(repo), Some(answered.clone())))
            .collect();
        state.set_settled(asked, None, false);

        for width in [34u16, 35, 38, 39] {
            let drawn = screen(&state, width, 16);
            let row = drawn
                .lines()
                .find(|line| line.contains("[x]") && line.contains("/w/x"))
                .unwrap_or_else(|| panic!("no marked row at {width}"));
            assert_eq!(
                row.contains("PR #1234 merged"),
                width >= 35,
                "the reason, at {width}: {row}"
            );
        }
    }

    #[test]
    fn the_sweep_says_it_is_still_asking_gh_before_it_says_what_it_found() {
        // Until `gh` answers the rows show what git alone decided, which is a smaller sweep
        // than the one about to arrive. Without the spinner that reads as a finished answer.
        let mut state = swept();
        state.set_settled(BTreeMap::new(), None, true);
        insta::assert_snapshot!(screen(&state, 92, 20));
    }

    #[test]
    fn a_gh_that_could_not_answer_says_why_once_and_which_rows_it_cost() {
        // The sentence goes on the prompt line because it is the one thing the rows cannot
        // say for themselves: they can say a checkout could not be judged, not why.
        let mut state = swept();
        let asked: BTreeMap<_, _> = state
            .tree()
            .repos
            .iter()
            .map(|repo| (RepoRoot::of(repo), None))
            .collect();
        state.set_settled(
            asked,
            Some("gh could not be run: no such file or directory".to_string()),
            false,
        );
        insta::assert_snapshot!(screen(&state, 92, 20));
    }

    #[test]
    fn a_row_marked_where_gh_could_not_look_goes_on_saying_so() {
        // Marking `chore/deps` turned `PR unknown` into `no pane` — the warning went away at
        // the moment it started to matter, and the row was then indistinguishable from one
        // marked by hand on a repository `gh` had answered for.
        let mut state = swept();
        let asked: BTreeMap<_, _> = state
            .tree()
            .repos
            .iter()
            .map(|repo| (RepoRoot::of(repo), None))
            .collect();
        state.set_settled(asked, Some("gh could not be run".to_string()), false);
        for _ in 0..20 {
            if label_under_cursor(&state) == "chore/deps" {
                break;
            }
            press(&mut state, KeyCode::Char('j'));
        }
        assert_eq!(label_under_cursor(&state), "chore/deps");
        press(&mut state, KeyCode::Char(' '));

        let drawn = screen(&state, 92, 20);
        let row = drawn
            .lines()
            .find(|line| line.contains("chore/deps"))
            .expect("the row is drawn");
        assert!(row.contains("[x]"), "marked: {row}");
        assert!(
            row.contains("PR unknown"),
            "and still says nobody could judge it: {row}"
        );
    }

    #[test]
    fn a_pull_request_gh_found_is_named_on_the_row_it_decided() {
        // `chore/deps` was staying a moment ago: git had nothing to say about it. `gh` may
        // only widen a sweep, and this is what widening looks like.
        let mut state = swept();
        let answered = SettledPullRequests::All(vec![SettledPullRequest {
            number: 123,
            head_ref: "chore/deps".to_string(),
            from_a_fork: false,
            outcome: PullRequestOutcome::Merged,
        }]);
        let asked: BTreeMap<_, _> = state
            .tree()
            .repos
            .iter()
            .map(|repo| (RepoRoot::of(repo), Some(answered.clone())))
            .collect();
        state.set_settled(asked, None, false);
        insta::assert_snapshot!(screen(&state, 92, 20));
    }

    #[test]
    fn every_checkout_says_what_state_it_is_in() {
        // The four answers, on four checkouts: ahead and behind its upstream, an upstream
        // that is gone, uncommitted work, and a checkout with nothing to report at all.
        let mut tree = tree();
        tree.repos[0].worktrees[0].track = Some(Track::Diverged {
            ahead: NonZeroU32::new(2).unwrap(),
            behind: NonZeroU32::new(1).unwrap(),
        });
        tree.repos[0].worktrees[1].track = Some(Track::Gone);
        tree.repos[0].worktrees[2].track = Some(Track::Behind(NonZeroU32::new(3).unwrap()));
        let mut state = PanesState::new(tree, None);
        state.set_working_trees(answers(&[
            ("/wt/feat-login", WorkingTree::Dirty),
            ("/wt/fix-crash", WorkingTree::Dirty),
        ]));
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
    fn the_question_names_every_pane_that_stops() {
        // A finished worktree has panes in it, so this is the ordinary shape of the
        // question rather than an unusual one. Uncommitted work is git's to protect and it
        // does; what a working agent has in flight has no other safety net than this list.
        let mut tree = tree();
        // A second pane in the same checkout, with no agent in it: the columns have to line
        // up, and a pane with nothing to report says nothing rather than `unknown`.
        tree.repos[0].worktrees[1]
            .panes
            .push(pane("w2:p2", None, AgentStatus::Unknown, false));
        let mut state = PanesState::new(tree, None);
        state.set_working_trees(answers(&[("/wt/feat-login", WorkingTree::Clean)]));
        // Onto `codex`, the first pane running in the `feat/login` checkout.
        for _ in 0..2 {
            press(&mut state, KeyCode::Char('j'));
        }
        press(&mut state, KeyCode::Char('D'));
        insta::assert_snapshot!(screen(&state, 92, 18));
    }

    #[test]
    fn a_short_pane_keeps_what_stops_and_gives_up_the_path() {
        // The path can be read from the breadcrumb behind the box. What is about to stop
        // cannot be read anywhere else, so it is the last thing to go.
        let mut state = PanesState::new(tree(), None);
        state.set_working_trees(answers(&[("/wt/feat-login", WorkingTree::Clean)]));
        for _ in 0..2 {
            press(&mut state, KeyCode::Char('j'));
        }
        press(&mut state, KeyCode::Char('D'));
        insta::assert_snapshot!(screen(&state, 92, 11));
    }

    #[test]
    fn a_pane_too_small_for_any_question_asks_none() {
        // The rung ladder decides what a box says; it cannot decide what the keyboard does
        // afterwards. So when nothing fits, drawing says so and the loop takes the question
        // back — otherwise `y` would be armed over a box nobody ever saw, and the key hint
        // at the bottom names the keys, never what they answer.
        let mut state = PanesState::new(tree(), None);
        state.set_working_trees(answers(&[("/wt/feat-login", WorkingTree::Clean)]));
        for _ in 0..2 {
            press(&mut state, KeyCode::Char('j'));
        }
        press(&mut state, KeyCode::Char('D'));
        assert!(state.pending_removal().is_some());

        // Two lines and a border is the floor, the same one a checkout with no panes has.
        let mut short = Terminal::new(TestBackend::new(92, 7)).unwrap();
        short
            .draw(|frame| {
                assert!(
                    !draw(frame, &state, &theme(), Mode::Panes),
                    "too short for even the question"
                );
            })
            .unwrap();

        // And too narrow, which the height ladder cannot see.
        let mut narrow = Terminal::new(TestBackend::new(24, 40)).unwrap();
        narrow
            .draw(|frame| {
                assert!(
                    !draw(frame, &state, &theme(), Mode::Panes),
                    "`Delete this checkout` clipped out of `… and close 1 pane?` is a \
                     complete sentence and a false one"
                );
            })
            .unwrap();
    }

    #[test]
    fn a_pane_too_short_for_the_names_still_says_how_many_stop() {
        // The rung below the list, where the question itself takes over the number. It is
        // two lines, the same as a checkout with no panes gets, so there is no height at
        // which `y` is armed over a box that never said panes would close.
        let mut state = PanesState::new(tree(), None);
        state.set_working_trees(answers(&[("/wt/feat-login", WorkingTree::Clean)]));
        for _ in 0..2 {
            press(&mut state, KeyCode::Char('j'));
        }
        press(&mut state, KeyCode::Char('D'));
        insta::assert_snapshot!(screen(&state, 92, 9));
    }

    #[test]
    fn a_working_tree_git_would_not_answer_for_says_so_on_its_own_row() {
        // Rows with no marker would otherwise read as clean working trees, which is a claim
        // rather than the absence of one. On the row rather than in a count, because a
        // count says how many and never which — and one prunable worktree is enough to
        // produce one, alongside rows that were answered for perfectly well.
        let mut state = PanesState::new(tree(), None);
        state.set_working_trees(answers(&[
            ("/wt/feat-login", WorkingTree::Dirty),
            ("/wt/fix-crash", WorkingTree::Unreadable),
        ]));
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
            .draw(|frame| {
                draw(frame, state, &theme(), Mode::Panes);
            })
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
            .draw(|frame| {
                draw(frame, &state, &theme, Mode::Panes);
            })
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
        state.set_working_trees(answers(&[
            ("/wt/fix-crash", WorkingTree::Dirty),
            ("/wt/main", WorkingTree::Dirty),
        ]));
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
