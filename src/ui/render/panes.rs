//! Drawing the panes picker: the search line, the rows, and the tree they are drawn as.

use super::prompt::{render_conditions, render_removal, render_sweep_removal};
use super::*;
use crate::ui::words;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::domain::rows::{DisplayLine, Row};
use crate::ui::panes::PanesState;
use crate::ui::theme::Theme;

/// Blank columns between the longest label and the meta column, so the two read as
/// neighbours rather than as one run of text.
const META_GAP: usize = 4;

/// The meta column never starts so far right that nothing useful fits after it.
const MIN_META_WIDTH: usize = 20;

/// The fewest columns a path keeps before the note beside the name is dropped instead.
///
/// Enough for `/…re-deps` — an ellipsis and a tail, and the tail is what tells checkouts
/// apart. A note is allowed past the meta column (see `MIN_LABEL`), but not this far: a row
/// carrying a mark and its reason and no path says nothing about which checkout either is
/// about.
const MIN_PATH: usize = 8;

/// The fewest columns a label keeps before the note beside it is dropped instead.
///
/// Enough for `fea…` — the start of a name and the ellipsis that says it is one.
///
/// What it may drop is `no pane`, the one note that is only ever a remark. It may not drop
/// `gone`, which is a marker rather than a note and is measured into the meta column, nor
/// `deleting`, nor what a sweep says beside a box — ADR 0011 asks for exactly that, since a
/// mark whose reason is invisible is one the user trusts blindly. Where those do not fit
/// beside the name, the name gives way instead, down to nothing, and the path in the meta
/// column is what still says which checkout the row is about — which is why past `MIN_PATH`
/// the note gives way after all.
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
/// **The meta column is a maximum over every row, so nothing that can appear while the picker
/// is up may make a row wider than it was measured.**
/// [`ui::words::marks_reserve`](crate::ui::words::marks_reserve) therefore keeps room
/// for the `✱` whether or not it is showing — `✱` and `?` are the same width, so one reserve
/// serves both. The `deleting` note is the deliberate exception: it appears on a keypress on
/// one row, and those columns come out of that row's own label rather than out of everyone
/// else's alignment.
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
        + words::row_label(row).chars().count()
        + words::marks_reserve(row)
        + if row.is_idle { IDLE_NOTE.len() } else { 0 }
}

/// `" ◆ "` or three spaces.
const GUTTER_WIDTH: usize = 3;

const IDLE_NOTE: &str = "  no pane";

/// A checkout being removed says so where its `no pane` note would go: it is the more
/// urgent fact about the same row, and the removal is running somewhere this picker cannot
/// see.
const REMOVING_NOTE: &str = "  deleting ";

/// What a row says about itself in a sweep — `PR #123 merged`, `PR unknown`, `refs
/// unreadable` — with the gap the other notes use.
///
/// Not `gone`, which the row already carries as its upstream marker, and not a refusal,
/// which is said by the absence of a box.
/// [`ui::words::sweep_note`](crate::ui::words::sweep_note) is where both of those
/// are decided and why.
///
/// Left out of `label_end` for the reason `REMOVING_NOTE` is: it arrives on `Shift-S` and
/// changes again when `gh` answers, and measuring it would move every path in the list
/// sideways. The columns come out of the label of the row that wanted them.
fn sweep_note(row: &Row) -> Option<String> {
    Some(format!("  {}", words::sweep_note(row.sweep.as_ref()?)?))
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
/// the two while the question on screen is what to delete.
///
/// As it stands the two never meet: [`domain::rows::flatten`](crate::domain::rows::flatten)
/// gives every worktree row `is_current: false`, so the order the gutter takes them in is
/// unobservable and a mutation that swaps them survives. Whoever makes a checkout able to be
/// current has to decide it, which is why the gutter matches on the pair rather than checking
/// one and falling through.
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
/// thing left, so a narrow pane loses `p no-pane` before it loses remove, states, or reload.
pub(crate) const HELP_PANES: &[&str] = &[
    "\u{21b5} jump  n new pane  p no-pane rows  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  shift+d remove  r reload  esc close",
    "\u{21b5} jump  n new pane  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  shift+d remove  r reload  esc close",
    "\u{21b5} jump  n new  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  shift+d remove  esc close",
    "\u{21b5} jump  n new  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  esc close",
    "\u{21b5} jump  n new  \u{2190}\u{2192} repo  \u{21e5} branches  / search  esc close",
    // Narrow enough that something has to go: the other view outranks a way of moving
    // around this one, which the arrow keys suggest on their own.
    "\u{21b5} jump  \u{21e5} branches  / search  esc close",
    "\u{21b5} jump  esc close",
];

/// While something is wrong. [`HELP_PANES`] with the key that reads it in full.
///
/// It outranks the reminder of `shift+d` from the rung below the widest: a key that has
/// been in the hint since the picker opened is one the user has already seen, and this one
/// has only just become worth pressing. It is also the last thing to go as the rungs
/// narrow, because the narrow end is where the line has given up its words for a count.
const HELP_PANES_TROUBLE: &[&str] = &[
    "\u{21b5} jump  n new pane  p no-pane rows  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  shift+d remove  r reload  ! what is wrong  esc close",
    "\u{21b5} jump  n new pane  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  shift+d remove  ! what is wrong  esc close",
    "\u{21b5} jump  n new  \u{2190}\u{2192} repo  \u{21e5} branches  / search  b/w/i/d/a states  ! what is wrong  esc close",
    "\u{21b5} jump  n new  \u{21e5} branches  / search  ! what is wrong  esc close",
    "\u{21b5} jump  \u{21e5} branches  ! what is wrong  esc close",
    "\u{21b5} jump  ! what is wrong  esc close",
    "! what is wrong  esc close",
];

/// While the panel holding every condition is up. The only keys it has are the ones that
/// close it, and `dump` is where the words are when the pane is too small even for this.
const HELP_PANES_CONDITIONS: &[&str] = &[
    "! or esc closes  herdr-worktree-nav dump has them in full",
    "! or esc closes",
];

/// While a deletion is waiting on a yes. Says what the dialog says, in case the dialog is
/// too small a pane to have drawn everything.
const HELP_PANES_REMOVE: &[&str] = &["y delete  any other key cancels", "y delete"];

/// While a sweep is on.
const HELP_PANES_SWEEP: &[&str] = &[
    "space mark  \u{2191}\u{2193} move  \u{2190}\u{2192} repo  \u{21b5} delete marked  shift+s done  esc done",
    "space mark  \u{2191}\u{2193} move  \u{21b5} delete  esc done",
    "space mark  \u{21b5} delete  esc done",
];

const HELP_PANES_SEARCH: &[&str] = &[
    "\u{21b5} keep search  ctrl+u clear  esc cancel  \u{2191}\u{2193} move  \u{2190}\u{2192} repo",
    "\u{21b5} keep  esc cancel",
];

/// `false` when a removal is waiting on an answer and this pane was too small to put the
/// question in. The loop takes the question back when that happens — see `render_removal`.
pub fn draw(frame: &mut Frame, state: &PanesState, theme: &Theme) -> bool {
    let Some(panel) = layout(frame) else {
        return state.pending_removal().is_none() && state.pending_sweep().is_none();
    };

    frame.render_widget(search_line(state, theme, panel.search.width), panel.search);
    render_rule(frame, panel.rule, theme);
    render_rows(frame, state, theme, panel.body);
    // One question at a time: `Shift-D` is not answered during a sweep, and only `y` on a
    // sweep's question ends the sweep.
    let asked = match (state.pending_removal(), state.pending_sweep()) {
        (Some(removal), _) => render_removal(frame, removal, state.home(), theme, panel.body),
        (None, Some(sweep)) => render_sweep_removal(frame, sweep, state.home(), theme, panel.body),
        (None, None) => {
            // Under a question rather than beside it: a question is answered with a key and
            // this is only read, so the one that can delete something owns the space.
            if state.is_showing_conditions() {
                render_conditions(frame, &state.conditions(), theme, panel.body);
            }
            true
        }
    };
    render_detail(frame, &state.detail(), theme, panel.detail);

    let variants = match (
        state.pending_removal().is_some() || state.pending_sweep().is_some(),
        state.is_sweeping(),
        state.is_filtering(),
    ) {
        (true, _, _) => HELP_PANES_REMOVE,
        // Before every other variant but a question's: the keys the list offers do nothing
        // while the panel is up, and a footer offering them would be describing a screen
        // the user is not on.
        _ if state.is_showing_conditions() => HELP_PANES_CONDITIONS,
        // Before the search variant, because `/` does nothing during a sweep — the keys a
        // footer offers have to be the keys that answer.
        (false, true, _) => HELP_PANES_SWEEP,
        (false, false, true) => HELP_PANES_SEARCH,
        // `!` is worth a place in the hint only when there is something to read, and it
        // survives every rung below: at the widths where the line itself has given up its
        // words for a count, the key that gets them back is the most useful thing left.
        (false, false, false) if !state.conditions().is_empty() => HELP_PANES_TROUBLE,
        (false, false, false) => HELP_PANES,
    };
    frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
    asked
}

/// `/ query` with the total on the right, or a state chip when one is active.
/// The fewest columns a condition keeps before it gives up its words for a count.
///
/// Enough for a repository's name and the ellipsis that says there was more. Below that the
/// words are not a sentence any more, and a bare count at least says how many things are
/// wrong — which is the one thing no width may leave unsaid.
const MIN_CONDITION: usize = 12;

/// What waits on an answer, drawn after the field. `labels` is given up before a condition
/// is, because a spinner says what it is with its glyph alone and a sentence does not.
fn waiting_on(state: &PanesState, theme: &Theme, labels: bool) -> Vec<Span<'static>> {
    let mut tail = Vec::new();
    // Whether a checkout is holding uncommitted work is a walk of its whole working tree,
    // one per checkout, so the answers land after the first frame. The spinner says the
    // list is still filling in rather than finished and empty-handed — the same thing the
    // branches view does while it waits on a remote.
    //
    // A checkout git would not answer for says so on its own row rather than here — see
    // `ui::words::marks`, and `docs/adr/0011-what-may-be-swept.md`, which puts the
    // unknown on the row it belongs to for the same reason.
    if state.is_waiting() {
        tail.push(Span::raw("  "));
        tail.push(Span::styled(spinner(state.frame()), theme.dim()));
        if labels {
            tail.push(Span::styled(" reading working trees\u{2026}", theme.dim()));
        }
    }
    // Its own spinner, because until `gh` answers the rows are showing what git alone
    // decided — a smaller sweep than the one the user is about to get, and one that is about
    // to change under their cursor.
    if state.is_asking_gh() {
        tail.push(Span::raw("  "));
        tail.push(Span::styled(spinner(state.frame()), theme.dim()));
        if labels {
            tail.push(Span::styled(" asking gh\u{2026}", theme.dim()));
        }
    }
    tail
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

/// `/ query` with the total on the right, a state chip where one is on, and what is wrong
/// beside all of them.
///
/// A condition does not take its turn behind the chip, the query or the placeholder: those
/// are how the picker is ordinarily used and it is a repository that has lost its markers,
/// so the two sit side by side and the line is divided between them. What it gives up first
/// is its own words — to a count, never to nothing — and before that the spinners give up
/// their labels. Issue #35 is what any of those going silent looked like.
fn search_line(state: &PanesState, theme: &Theme, width: u16) -> Paragraph<'static> {
    let focus = if state.is_filtering() {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.dim()
    };
    let mut spans = vec![Span::styled(" / ", focus)];

    // During a sweep the number being decided about is how many are going, not how many
    // panes are open.
    let count = if state.is_sweeping() {
        format!("{} marked", state.marked_count())
    } else {
        format!("{} panes", state.pane_count())
    };

    let conditions = state.conditions();
    // Not while the field has the cursor: there the user is reading what they are typing,
    // and a sentence arriving beside it mid-word is the one moment the field really is
    // theirs. A query they have kept is a different thing — they are back to reading the
    // list — and issue #35 asks for it there.
    let condition = if state.is_filtering() {
        None
    } else {
        words::conditions_line(&conditions)
    };
    // Two columns of gap before it, so it reads as its own thing beside the field.
    let badge = format!("!{}", conditions.len());
    let reserved = condition.as_ref().map_or(0, |_| badge.chars().count() + 2);

    // The labelled spinners only where a condition would still have words beside them. A
    // spinner says what it is with its glyph alone, so its label is the cheaper thing to
    // give up, and giving it up first is what issue #35 asks for. With nothing wrong there
    // is nothing to weigh it against and the labels stay.
    let width = width as usize;
    let spare = |tail: &[Span<'static>]| {
        width.saturating_sub(spans_width(&spans) + spans_width(tail) + count.chars().count() + 3)
    };
    let labelled = waiting_on(state, theme, true);
    let wanted = condition.as_ref().map_or(0, |_| MIN_CONDITION + 2);
    let tail = if spare(&labelled) >= wanted {
        labelled
    } else {
        waiting_on(state, theme, false)
    };
    let room = spare(&tail);

    // What the field itself holds. Cut to what is left once the condition beside it has been
    // kept room for, so a long toast cannot take the line whole.
    let left_room = room.saturating_sub(reserved);
    let before = spans_width(&spans);
    if let Some(message) = state.message() {
        // What git said, or a toast, can be as long as the source made it. The words that
        // fit are the start of it, and an ellipsis says there was more.
        spans.push(Span::styled(
            truncate(message, left_room),
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
                words::state_filter(filter),
                style.add_modifier(Modifier::BOLD),
            ));
        }
        if !state.query().is_empty() {
            if state.state_filter().is_some() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::raw(truncate(state.query(), left_room)));
        } else if state.state_filter().is_none() && !state.is_filtering() && condition.is_none() {
            // The placeholder is what to do when the field is not focused; once it is, the
            // cursor says everything and the hint is in the way of what is being typed. It
            // is also the one thing a condition does displace outright: a hint the reader
            // could guess is worth less than a repository that has lost its markers.
            //
            // `/` does nothing during a sweep, so there the field says what the mode is
            // instead of offering a search that would not run.
            let hint = if state.is_sweeping() {
                "sweep"
            } else {
                "search panes"
            };
            spans.push(Span::styled(hint, theme.dim()));
        }
    }
    if state.is_filtering() {
        spans.push(Span::styled("\u{2588}", theme.dim()));
    }

    if let Some(condition) = condition {
        let used = spans_width(&spans) - before;
        // The gap only where there is something to be separated from.
        let gap = if used == 0 { 0 } else { 2 };
        let left = room.saturating_sub(used + gap);
        if left > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(Span::styled(
                if left >= MIN_CONDITION {
                    truncate(&condition, left)
                } else {
                    truncate(&badge, left)
                },
                theme.dim(),
            ));
        }
    }
    spans.extend(tail);
    let used = spans_width(&spans);
    let pad = width.saturating_sub(used + count.chars().count() + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(count, theme.dim()));
    Paragraph::new(Line::from(spans))
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
    let marks = words::marks(row);
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
    // and the number is the checkable part. `no pane` is a remark about a checkout the user
    // may still act on, and the name is worth more than the remark.
    //
    // Two notes never give way. `deleting` is the whole of what the picker adds over the
    // toast (`docs/adr/0014-removing-outlives-the-picker.md`) on a row the cursor will not
    // stop on and nothing can be done to. A sweep's note is the reason for the box in the
    // gutter, which ADR 0011 says may not be shown without one, on the list `Enter` will act
    // on. The name gives way instead, down to nothing: the path still says which checkout it
    // is.
    let with_note = room.saturating_sub(note_width(row));
    // But never past the path. A note wider than the room is drawn on into the meta column,
    // so it is kept only while the path keeps `MIN_PATH` columns after it. Narrower than
    // that the name gets the room back: a `[x]` with a reason and nothing the reason is
    // about is worse than a `[x]` with a name and no reason.
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
        Span::styled(truncate(&words::row_label(row), label_budget), label_style),
    ];
    // Beside the name rather than in a column, because most rows have none of it.
    if !marks.is_empty() {
        spans.push(Span::styled(marks, quiet));
    }
    // The meta column is taken by the checkout path, so a checkout with nothing running in
    // it says so beside its name instead.
    if !keeps_its_note {
        // The name won the columns. Nothing goes here.
    } else if row.is_removing {
        spans.push(Span::styled(REMOVING_NOTE, quiet));
        spans.push(Span::styled(spinner(tick), quiet));
    } else if let Some(note) = sweep_note(row) {
        // Why this row is going, or why it cannot — `docs/adr/0011-what-may-be-swept.md`.
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

/// Tree prefix for a row, trailing space included: connected branch glyphs, with `│`
/// continuations under ancestors that still have siblings below.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::plugin_config;
    use crate::domain::chrome::Chrome;
    use crate::domain::model::{Branch, CheckoutPath, Position, WorkingTree};
    use crate::ui::render::fixtures::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;
    use ratatui::Terminal;
    use std::collections::BTreeMap;
    use std::num::NonZeroU32;

    use crate::domain::model::Refs;

    use crate::domain::sweep::RepoRoot;
    use crate::port::Track;
    use crate::port::{PullRequestOutcome, SettledPullRequest, SettledPullRequests};

    #[test]
    fn a_repository_whose_refs_git_would_not_read_is_named_where_the_search_hint_was() {
        // Its rows carry no marker, which is what they carry when there is nothing to
        // report; the prompt line is the one place that says which of the two it is.
        let mut tree = tree();
        tree.repos[0].refs = Refs::Unreadable(REFS_REFUSAL.into());
        let mut state = PanesState::new(tree, None);
        insta::assert_snapshot!(screen(&state, 92, 18));

        // Typing takes the field back: the hint gives way to the query, and so does this.
        press(&mut state, KeyCode::Char('/'));
        press(&mut state, KeyCode::Char('m'));
        let typed = screen(&state, 92, 18);
        assert!(
            !typed.contains("refs unreadable"),
            "the search field is the user's while they type:\n{typed}"
        );
    }

    #[test]
    fn the_refs_sentence_shares_the_line_with_a_state_chip_and_with_a_message() {
        // Two other things sit in the field, and neither takes it whole. A chip stays until
        // the filter is cleared, which is an ordinary way to use the picker rather than a
        // moment, and a message is there for one keypress — and all the while a repository
        // is missing its markers with nothing on its rows to say so. Issue #35.
        let mut tree = tree();
        tree.repos[0].refs = Refs::Unreadable("fatal: bad ref".into());
        let mut state = PanesState::new(tree, None);

        press(&mut state, KeyCode::Char('b'));
        let filtered = screen(&state, 92, 18);
        let line = filtered.lines().next().expect("the prompt line");
        assert!(line.contains("blocked"), "the chip is there: {line}");
        assert!(
            line.contains("refs unreadable"),
            "and so is the sentence, beside it: {line}"
        );
        press(&mut state, KeyCode::Char('a'));
        let cleared = screen(&state, 92, 18);
        assert!(
            cleared.lines().next().unwrap().contains("refs unreadable"),
            "and it is still there once the filter is gone: {cleared}"
        );

        state.set_message("select a worktree or a pane first".into());
        let told = screen(&state, 92, 18);
        let line = told.lines().next().expect("the prompt line");
        assert!(line.contains("select a worktree"), "the message: {line}");
        assert!(
            line.contains("refs unreadable"),
            "and the sentence keeps its room beside it, because a message is a moment and \
                 this is not: {line}"
        );
    }

    #[test]
    fn a_second_dropped_ref_reaches_the_line_once_there_is_room_for_it() {
        // The prompt line cuts from the right, so the second refname arrives only once the
        // line has room for the whole of the first.
        let mut tree = tree();
        tree.repos[0].refs = Refs::Unreadable(TWO_REFS_REFUSAL.into());
        let state = PanesState::new(tree, None);

        let narrow = screen(&state, 92, 18);
        let line = narrow.lines().next().expect("the prompt line");
        assert!(line.contains("refs/heads/main"), "the first ref: {line}");
        assert!(
            !line.contains("refs/heads/chore/deps"),
            "and nothing of the second, at 92: {line}"
        );

        // The narrow assertion above holds under any cut. The pair of widths one apart is
        // what pins this one to the right cut.
        let one_short = screen(&state, 132, 18);
        let line = one_short.lines().next().expect("the prompt line");
        assert!(
            !line.contains("refs/heads/chore/deps"),
            "132 is one short: {line}"
        );

        let wide = screen(&state, 133, 18);
        let line = wide.lines().next().expect("the prompt line");
        assert!(
            line.contains("refs/heads/main") && line.contains("refs/heads/chore/deps"),
            "both, at 133: {line}"
        );
    }

    #[test]
    fn appending_the_call_costs_the_second_refname_the_ellipsis_and_no_more() {
        // `refusal` appends the call after both refnames and the prompt line cuts from the
        // right, so carrying the call costs the second refname only the ellipsis. It makes
        // the sentence several times as long, which is what it costs the reader — but reach
        // is not what it costs, and that is the part a reader putting the call back would
        // guess wrong.
        let with_the_call = format!(
            "{TWO_REFS_REFUSAL} (`git for-each-ref \
                 --format=%(refname)%09%(committerdate:unix)%09%(upstream:short)%09\
                 %(upstream:track)%09%(push:track)%09%(worktreepath)%09%(contents:subject) \
                 refs/heads refs/remotes`)"
        );
        let mut wordier = tree();
        wordier.repos[0].refs = Refs::Unreadable(with_the_call);
        let state = PanesState::new(wordier, None);

        let still_narrow = screen(&state, 133, 18);
        let line = still_narrow.lines().next().expect("the prompt line");
        assert!(
            !line.contains("refs/heads/chore/deps"),
            "with the call there, 133 is one short: {line}"
        );
        let one_wider = screen(&state, 134, 18);
        let line = one_wider.lines().next().expect("the prompt line");
        assert!(
            line.contains("refs/heads/chore/deps"),
            "and 134 reaches it, which is the whole of what the call cost: {line}"
        );
    }

    #[test]
    fn panes_help_drops_p_before_remove_states_or_reload() {
        // Same pick as `footer`: first rung whose length is strictly less than the width.
        for text in super::HELP_PANES {
            assert!(
                !text.contains("branch") || text.contains("branches"),
                "no singular branch: {text}"
            );
        }
        for width in 18..=140u16 {
            let text = panes_help(width);
            if width >= 121 {
                assert!(text.contains("no-pane"), "p fits from 121: {width} {text}");
                assert!(
                    text.contains("reload"),
                    "reload stays on the p rung: {width} {text}"
                );
            } else {
                assert!(
                    !text.contains("no-pane"),
                    "p is gone before reload/remove/states: {width} {text}"
                );
            }
            if width >= 105 {
                assert!(text.contains("reload"), "reload from 105: {width} {text}");
            }
            if width >= 90 {
                assert!(text.contains("remove"), "remove from 90: {width} {text}");
            }
            if width >= 74 {
                assert!(
                    text.contains("b/w/i/d/a states"),
                    "states from 74: {width} {text}"
                );
            }
        }
    }

    #[test]
    fn a_config_complaint_fits_on_the_prompt_line_and_the_count_survives_it() {
        // Seeded as the loader would. The reason has to show as soon as there is room for
        // it, not well after.
        let mut state = PanesState::new(tree(), None);
        state.set_message(plugin_config::complaint_for(
            "[pane]\nshow_worktrees_without_panes = false\n",
        ));
        for width in 24..=92u16 {
            let line = prompt_line(&state, width);
            assert!(
                line.trim_end().ends_with("5 panes"),
                "the count is still there at {width}: {line}"
            );
            if width >= 54 {
                assert!(
                    line.contains("unknown field `pane`"),
                    "the reason is what fits at {width}: {line}"
                );
            }
        }
    }

    #[test]
    fn a_near_miss_key_still_names_the_wrong_key_on_a_narrow_prompt_line() {
        // The likeliest typo is a missing `s`. Serde names both keys in full, so this is
        // the long complaint; the wrong key has to stay visible where the right one is cut.
        let mut state = PanesState::new(tree(), None);
        state.set_message(plugin_config::complaint_for(
            "[panes]\nshow_worktrees_without_pane = false\n",
        ));
        for width in 24..=92u16 {
            let line = prompt_line(&state, width);
            assert!(
                line.trim_end().ends_with("5 panes"),
                "the count is still there at {width}: {line}"
            );
            if width >= 77 {
                assert!(
                    line.contains("show_worktrees_without_pane"),
                    "the mistyped key is what fits at {width}: {line}"
                );
            }
        }
        let just_short = prompt_line(&state, 116);
        assert!(
            !just_short.contains("expected `show_worktrees_without_panes`"),
            "one column short, the right key is still cut: {just_short}"
        );
        let wide = prompt_line(&state, 117);
        assert!(
            wide.contains("expected `show_worktrees_without_panes`"),
            "with room, the right key is there too: {wide}"
        );
    }

    #[test]
    fn no_width_leaves_the_prompt_line_silent_about_a_repository_in_trouble() {
        // The half of issue #35 that was a blank screen rather than a precedence: at three
        // of the widths the picker supports, `truncate` was handed nothing to work with and
        // printed nothing at all — not even the ellipsis that says something was cut. What
        // has to hold at every width is that something is there, whether that is git's
        // words, a cut of them, or a bare count of how many things are wrong.
        let mut tree = tree();
        tree.repos[0].refs = Refs::Unreadable(REFS_REFUSAL.into());
        let mut state = PanesState::new(tree, None);
        for waiting in [false, true] {
            state.set_waiting(waiting);
            for width in 24..=92u16 {
                let line = prompt_line(&state, width);
                assert!(
                    line.contains("me/app") || line.contains('!') || line.contains('\u{2026}'),
                    "something says a repository is in trouble at {width} \
                         (waiting: {waiting}): {line:?}"
                );
            }
        }
    }

    #[test]
    fn gits_words_fit_on_the_prompt_line_and_the_count_survives_them() {
        // The sentence is as long as git made it, and what fits has to be git's words
        // rather than the plugin's argv. Walked with and without the spinner, which shares
        // the line.
        let mut tree = tree();
        tree.repos[0].refs = Refs::Unreadable(REFS_REFUSAL.into());
        let mut state = PanesState::new(tree, None);
        for waiting in [false, true] {
            state.set_waiting(waiting);
            for width in 24..=92u16 {
                let line = prompt_line(&state, width);
                // Below this the spinner takes enough of the line that it, and not the cut,
                // is what pushes the count off — which is not what this measures.
                const COUNT_KEPT_BESIDE_THE_SPINNER: u16 = 36;
                if !waiting || width >= COUNT_KEPT_BESIDE_THE_SPINNER {
                    assert!(
                        line.trim_end().ends_with("5 panes"),
                        "the count is still there at {width}: {line}"
                    );
                }
                // From here up the line has room for the sentence as far as git's verdict;
                // below it the cut reaches into git's words. Measured without the spinner,
                // which would put the same point past the widest width this walks.
                const VERDICT_WHOLE: u16 = 67;
                if !waiting && width >= VERDICT_WHOLE {
                    assert!(
                        line.contains("ignoring broken ref"),
                        "git's words are what fits at {width}: {line}"
                    );
                }
                // The spinner's label is the first thing given up, and it comes back only
                // once the sentence can still have words beside it. Below this the glyph
                // goes on turning and the line spends its columns on git's words instead —
                // which is the trade issue #35 asks for, in that direction.
                const LABEL_BESIDE_THE_SENTENCE: u16 = 53;
                if waiting {
                    assert!(
                        line.contains(spinner(state.frame())),
                        "the spinner is turning at {width}: {line}"
                    );
                    assert_eq!(
                        line.contains("reading working trees"),
                        width >= LABEL_BESIDE_THE_SENTENCE,
                        "and it has its label only where the sentence keeps its own words \
                             at {width}: {line}"
                    );
                }
            }
        }
    }

    #[test]
    fn draws_the_tree_the_gutter_and_the_meta_column() {
        // Tall enough for the whole list, including the panes that are in no repository.
        insta::assert_snapshot!(screen(&PanesState::new(tree(), None), 92, 18));
    }

    #[test]
    fn a_sweep_puts_a_box_in_the_gutter_and_the_reason_beside_the_name() {
        // Every row says what is happening to it, and the count on the right is how many
        // are going rather than how many panes are open.
        insta::assert_snapshot!(screen(&swept(), 92, 20));
    }

    #[test]
    fn a_mark_is_drawn_as_a_mark_and_a_refusal_is_not_drawn_as_one() {
        // The gutter's four meanings — ticked, empty, no box, and the diamond where the
        // session is — would be drawn identically in every snapshot here if the colour were
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
        // is: the two share the columns and the colour, and outside a sweep the diamond is
        // the only thing that wears it.
        let mut ordinary = PanesState::new(tree(), None);
        // Off the focused pane, which would be drawn in the selection's colours.
        press(&mut ordinary, KeyCode::Char('j'));
        assert_eq!(
            cell_style(&ordinary, 92, 18, 1, line_of(&ordinary, "claude")).fg,
            accent,
            "the row the session is on says so, sweep or no sweep"
        );
    }

    #[test]
    fn a_removal_in_flight_says_so_at_every_width_the_picker_supports() {
        // Without the note a removal the picker cannot see is drawn as a perfectly ordinary
        // row — no note, no spinner — while the cursor silently will not stop on it and
        // `Shift-D` cannot reach it.
        let mut state = PanesState::new(tree(), None);
        state.set_removing(vec![CheckoutPath::for_test("/wt/fix-crash")]);
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
        // A sweep's reason is drawn without being measured, so it comes out of this row's
        // own label, and here there is not a name's worth and a reason's worth both. The
        // reason wins: ADR 0011 does not allow a box without its reason showing. The path
        // is the last thing on the row to be given away.
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
        // gives up, the reason for its box is not it.
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
        // The rule: a note may never take a path's last `MIN_PATH` columns, so each note
        // waits for the width where its own length leaves the path that much beside it.
        // Walked rather than reasoned about, because that width falls out of how long the
        // note is, and the two here are different lengths.
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
        removing.set_removing(vec![CheckoutPath::for_test("/wt/fix-crash")]);

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
            // Whole from where the note leaves it the room, and again below the width the
            // note arrives at, where there is no note to make room for.
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
        // `MIN_PATH` is a floor for paths with that much to show, not a toll every path
        // pays. A path shorter than the floor is whole in its own length, so holding its
        // note back until the whole floor was free would drop the reason for nothing.
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
        // The rows show what git alone decided; without the spinner that reads as a
        // finished answer.
        let mut state = swept();
        state.set_settled(BTreeMap::new(), None, true);
        insta::assert_snapshot!(screen(&state, 92, 20));
    }

    #[test]
    fn a_gh_that_could_not_answer_says_why_once_and_which_rows_it_cost() {
        // The prompt line carries it: the rows can say a checkout went unjudged, never why.
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
        // A row whose warning went away on being marked is indistinguishable from one
        // marked by hand on a repository `gh` answered for.
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
    fn a_row_in_a_repository_whose_refs_git_would_not_read_says_so_in_a_sweep() {
        // The prompt line names the repository; the row says which half of the question
        // went unanswered, and goes on saying it once marked — the same rule `PR unknown`
        // is under. The row is found by its path's tail because here the note takes the
        // label's place.
        let mut state = swept();
        let mut tree = state.tree().clone();
        tree.repos[1].refs = Refs::Unreadable("fatal: bad ref".into());
        state.replace_tree(tree);
        for _ in 0..20 {
            if label_under_cursor(&state) == "chore/deps" {
                break;
            }
            press(&mut state, KeyCode::Char('j'));
        }
        assert_eq!(label_under_cursor(&state), "chore/deps");

        let row_of = |drawn: &str| {
            drawn
                .lines()
                .find(|line| line.contains("chore-deps") && !line.starts_with(" me/site"))
                .expect("the row is drawn")
                .to_string()
        };
        for width in [92, 46] {
            let drawn = screen(&state, width, 20);
            let row = row_of(&drawn);
            assert!(
                row.contains("[ ]"),
                "unjudged is not offered at {width}: {row}"
            );
            assert!(row.contains("refs unreadable"), "at {width}: {row}");
            assert!(
                drawn.contains("me/site: refs unreadable: fatal"),
                "named once, on the prompt line, at {width}:\n{drawn}"
            );
        }

        press(&mut state, KeyCode::Char(' '));
        let row = row_of(&screen(&state, 92, 20));
        assert!(row.contains("[x]"), "marked: {row}");
        assert!(row.contains("refs unreadable"), "and still unjudged: {row}");
    }

    #[test]
    fn a_pull_request_gh_found_is_named_on_the_row_it_decided() {
        // `chore/deps` was staying: git had nothing to say about it. `gh` may only widen a
        // sweep, and this is what widening looks like.
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
        tree.repos[0].worktrees[0].position = Position::Said(Some(Track::Diverged {
            ahead: NonZeroU32::new(2).unwrap(),
            behind: NonZeroU32::new(1).unwrap(),
        }));
        tree.repos[0].worktrees[1].position = Position::Said(Some(Track::Gone));
        tree.repos[0].worktrees[2].position =
            Position::Said(Some(Track::Behind(NonZeroU32::new(3).unwrap())));
        let mut state = PanesState::new(tree, None);
        state.set_working_trees(answers(&[
            ("/wt/feat-login", WorkingTree::Dirty),
            ("/wt/fix-crash", WorkingTree::Dirty),
        ]));
        insta::assert_snapshot!(screen(&state, 92, 18));
    }

    #[test]
    fn nothing_is_claimed_about_a_working_tree_that_has_not_been_read_yet() {
        // Asking costs a process per checkout, so the answers land after the first frame,
        // and until one does the row says nothing about uncommitted work.
        let mut state = PanesState::new(tree(), None);
        state.set_waiting(true);
        insta::assert_snapshot!(screen(&state, 92, 18));
    }

    #[test]
    fn a_working_tree_git_would_not_answer_for_says_so_on_its_own_row() {
        // Rows with no marker would otherwise read as clean working trees, which is a claim
        // rather than the absence of one. On the row rather than in a count, because a count
        // says how many and never which.
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
        // the row has to say what is happening to it rather than simply going quiet.
        let mut state = PanesState::new(tree(), None);
        state.set_removing(vec![CheckoutPath::for_test("/wt/fix-crash")]);
        insta::assert_snapshot!(screen(&state, 92, 18));
    }

    #[test]
    fn an_empty_search_field_with_the_keyboard_shows_only_its_cursor() {
        // Leaving the placeholder under the cursor would read as text that will not go
        // away.
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('/'));
        insta::assert_snapshot!(screen(&state, 92, 6));
    }

    #[test]
    fn a_state_chip_and_a_typed_query_sit_beside_each_other() {
        // A chip drawn in the query's place is a field the letters go into with nothing
        // appearing.
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

    #[test]
    fn a_row_kept_only_as_context_is_dimmed_and_a_result_is_not() {
        let mut state = PanesState::new(tree(), None);
        press(&mut state, KeyCode::Char('/'));
        for c in "codex".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        let buffer = buffer_of(&state, 92, 12);
        // Snapshots record glyphs, not styles, so this is where the difference between a
        // result and the context around it is checked.
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
                draw(frame, &state, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        // No heading is ever under the cursor, so both keep the accent foreground.
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
        // `fix/crash`, its "no pane" note, and the columns kept for a `✱` that has not
        // arrived yet. Nothing else in this tree has anything to line up with.
        assert_eq!(meta_column(state.rows(), 92), 30 + META_GAP);
    }

    #[test]
    fn starting_a_removal_does_not_move_the_meta_column() {
        // A removal starts on a keypress, and a list that shifts sideways under one is a
        // list nobody can read while tidying up.
        let mut state = PanesState::new(tree(), None);
        let before = meta_column(state.rows(), 92);
        state.set_removing(vec![CheckoutPath::for_test("/wt/fix-crash")]);
        assert_eq!(meta_column(state.rows(), 92), before);
    }

    #[test]
    fn an_answer_about_uncommitted_work_does_not_move_the_meta_column_either() {
        // The same rule, with the answers arriving a beat after the first frame, when the
        // list is already on screen and being read.
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
        let before = meta_column(PanesState::new(tree(), None).rows(), 92);
        let mut wide = tree();
        wide.repos[0].display_name = "a-very-long-organisation/and-repository-name".into();
        let after = meta_column(PanesState::new(wide, None).rows(), 92);
        assert_eq!(before, after);
    }

    #[test]
    fn the_column_stops_short_of_the_edge_so_something_always_fits_after_it() {
        let mut tree = tree();
        tree.repos[0].worktrees[0].branch = Branch::Out("a".repeat(80));
        let state = PanesState::new(tree, None);
        assert_eq!(meta_column(state.rows(), 60), 60 - MIN_META_WIDTH);
    }

    // ---- branches view ----
}
