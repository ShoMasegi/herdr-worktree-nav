//! The layer the panes picker puts over its own rows: the question a removal asks, the one
//! a sweep asks, and the panel that reads out the conditions in full.

use super::*;
use crate::ui::words;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::domain::notice;
use crate::domain::removal::{Removal, SweepRemoval};
use crate::domain::rows::{abbreviate, UNNAMED_PANE};
use crate::ui::theme::Theme;

/// The keys under either question, and the one that answers it.
const KEYS_Y: &str = "y delete";

const KEYS_REST: &str = "     any other key cancels";

fn keys_line(theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            KEYS_Y,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(KEYS_REST, theme.dim()),
    ])
}

/// A question box's own columns: a border and a padding column on each side, and the two
/// that the lines inside it are indented by. The keys line is the one line measured without
/// the indent it is drawn with.
/// `a_question_box_is_its_longest_line_and_what_the_box_spends` is where the number is.
const BOX_CHROME: usize = 6;

/// The widest a question box gets, however long the line inside it is. A worktree path is
/// long enough to turn a dialog into a banner across a wide pane; what will not fit loses
/// its middle, and the breadcrumb under the list still carries the whole thing.
const BOX_MAX_WIDTH: usize = 80;

/// How wide a question box whose longest line is `widest` gets over `body`.
fn question_width(widest: usize, body: Rect) -> u16 {
    (widest + BOX_CHROME)
        .min(body.width as usize)
        .min(BOX_MAX_WIDTH) as u16
}

/// Draw the first of `candidates` that fits, centred over `body`, and say whether one did.
///
/// The candidates run from the roomiest to the barest. `false` when none of them fits, or
/// when `title_width` will not: a box too narrow for the question is as bad as no box —
/// `Delete this checkout and close 2 panes?` clipped to `Delete this checkout` is a complete
/// sentence and a false one. The caller takes a `false` as "this cannot be asked", and
/// cancels. `width` is [`question_width`]'s, which is what keeps it inside `body`.
fn question_box(
    frame: &mut Frame,
    theme: &Theme,
    body: Rect,
    width: u16,
    title_width: usize,
    candidates: Vec<Vec<Line>>,
) -> bool {
    if width < 8 || (width as usize) < title_width + BOX_CHROME {
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

/// The question a deletion asks, as a box over the list.
///
/// A dialog rather than a line in the search field: this is the one thing the picker does
/// that cannot be undone by doing it again.
///
/// `false` when the question could not be drawn at all. The caller must then take the
/// question back: a picker that leaves `y` armed over a box nobody saw is asking a question
/// it never put on screen.
pub(super) fn render_removal(
    frame: &mut Frame,
    removal: &Removal,
    home: Option<&str>,
    theme: &Theme,
    body: Rect,
) -> bool {
    const TITLE: &str = "Delete this checkout?";
    const CLOSING: &str = "  these panes close:";

    let path = abbreviate(removal.checkout_path().as_str(), home);
    // Uncommitted work is git's to protect and it does; what a working agent has in flight
    // has no other safety net, so the question names every pane that stops, in the words the
    // list behind the box uses for them.
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
    let width = question_width(widest, body);

    let blank = Line::from("");
    let title = Line::from(Span::styled(
        TITLE,
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let branch = Line::from(Span::raw(format!("  {}", removal.label())));
    let inner_width = (width as usize).saturating_sub(BOX_CHROME);
    let path = Line::from(Span::styled(
        format!("  {}", middle_elide(&path, inner_width)),
        theme.dim(),
    ));
    let keys = keys_line(theme);

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
    // question rather than a broken box. A branch and a path can be read from the breadcrumb
    // behind the box; the panes cannot be read anywhere, so they outlast both. On the last
    // rung the question itself takes over their number, so there is no height at which `y`
    // is armed over a box that never said panes would close.
    let counted_line = Line::from(Span::styled(
        counted.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let spaced = match panes.is_empty() {
        true => vec![blank.clone()],
        false => [vec![blank.clone()], panes.clone(), vec![blank.clone()]].concat(),
    };
    let candidates = vec![
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
    question_box(
        frame,
        theme,
        body,
        width,
        counted.chars().count(),
        candidates,
    )
}

/// What a sweep's question starts with: the count, and how many branches go after.
fn sweep_title(sweep: &SweepRemoval) -> String {
    let checkouts = match sweep.len() {
        1 => "1 checkout".to_string(),
        many => format!("{many} checkouts"),
    };
    match (sweep.len(), sweep.branches()) {
        (_, 0) => format!("Delete {checkouts}?"),
        (1, _) => format!("Delete {checkouts} and its branch?"),
        (all, branches) if all == branches => format!("Delete {checkouts} and their branches?"),
        (_, 1) => format!("Delete {checkouts} and 1 branch?"),
        (_, branches) => format!("Delete {checkouts} and {branches} branches?"),
    }
}

/// The question a sweep asks: the same box as `render_removal`, under the same contract —
/// `false` when it could not be drawn, and the caller takes the question back. A row per
/// checkout, as many as fit, then `+N more`; the title carries the count, so the smallest
/// box still says how many go.
pub(super) fn render_sweep_removal(
    frame: &mut Frame,
    sweep: &SweepRemoval,
    home: Option<&str>,
    theme: &Theme,
    body: Rect,
) -> bool {
    let title = sweep_title(sweep);
    let label_column = sweep
        .removals()
        .iter()
        .map(|removal| removal.label().chars().count())
        .max()
        .unwrap_or(0);
    let paths: Vec<String> = sweep
        .removals()
        .iter()
        .map(|removal| abbreviate(removal.checkout_path().as_str(), home))
        .collect();
    let widest = [
        title.chars().count(),
        KEYS_Y.chars().count() + KEYS_REST.chars().count(),
    ]
    .into_iter()
    .chain(
        paths
            .iter()
            .map(|path| label_column + path.chars().count() + 4),
    )
    .max()
    .unwrap_or(0);
    let width = question_width(widest, body);
    let inner_width = (width as usize).saturating_sub(BOX_CHROME);
    let path_budget = inner_width.saturating_sub(label_column + 2);

    let blank = Line::from("");
    let title_width = title.chars().count();
    let title = Line::from(Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let keys = keys_line(theme);
    let rows: Vec<Line> = sweep
        .removals()
        .iter()
        .zip(&paths)
        .map(|(removal, path)| {
            Line::from(vec![
                Span::raw(format!("  {}  ", pad(removal.label(), label_column))),
                Span::styled(middle_elide(path, path_budget), theme.dim()),
            ])
        })
        .collect();

    // The air goes first, then rows from the bottom with their number in their place, and
    // last the rows altogether: the title has said how many the whole time.
    let mut candidates = vec![
        [
            vec![title.clone(), blank.clone()],
            rows.clone(),
            vec![blank, keys.clone()],
        ]
        .concat(),
        [vec![title.clone()], rows.clone(), vec![keys.clone()]].concat(),
    ];
    for shown in (1..rows.len()).rev() {
        let more = Line::from(Span::styled(
            format!("  +{} more", rows.len() - shown),
            theme.dim(),
        ));
        candidates.push(
            [
                vec![title.clone()],
                rows[..shown].to_vec(),
                vec![more, keys.clone()],
            ]
            .concat(),
        );
    }
    candidates.push(vec![title, keys]);
    question_box(frame, theme, body, width, title_width, candidates)
}

/// A panel's own columns: a border and a padding column on each side. Unlike a question
/// box its lines are not indented — there is nothing above them to hang an indent off.
const PANEL_CHROME: usize = 4;

/// The fewest columns the panel is worth drawing in. Below it the prompt line's count is
/// what a pane that small gets, and `dump` is where the words are.
const PANEL_MIN_WIDTH: usize = 8;

/// Everything that is wrong, whole, as a panel over the list.
///
/// The prompt line has one line and gives its words up for a count as it narrows, so a
/// narrow pane cuts sentences nobody can then read. `dump` is the unabridged copy for a bug
/// report; this is the one for the person looking at the picker now, and `!` is what opens
/// it.
///
/// It degrades rather than refuses: what does not fit becomes a count of what is left, the
/// same answer the line gives for the same reason. A question box refuses instead, because
/// a clipped question is a different question — this is only ever read.
pub(super) fn render_conditions(
    frame: &mut Frame,
    conditions: &[notice::Condition],
    theme: &Theme,
    body: Rect,
) {
    if body.height == 0 || (body.width as usize) < PANEL_MIN_WIDTH {
        return;
    }
    // Room for a border costs two rows and two columns; under that the panel is drawn bare.
    let bordered = body.height >= 3;
    let width = (body.width as usize).min(BOX_MAX_WIDTH);
    let inner = match bordered {
        true => width - PANEL_CHROME,
        false => width,
    };

    let title = format!(
        "{} wrong",
        count_of(conditions.len(), "thing is", "things are")
    );
    let wrapped: Vec<Vec<String>> = conditions
        .iter()
        .map(|condition| wrap(&words::condition(condition), inner))
        .collect();

    let blank = Line::from("");
    let title = Line::from(Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let paragraphs: Vec<Vec<Line>> = wrapped
        .iter()
        .map(|lines| {
            lines
                .iter()
                .map(|line| Line::from(Span::raw(line.clone())))
                .collect()
        })
        .collect();

    // Air between one sentence and the next goes first, then sentences from the bottom with
    // a count in their place. The title has said how many there are the whole time.
    let mut airy = vec![title.clone(), blank.clone()];
    for (index, paragraph) in paragraphs.iter().enumerate() {
        airy.extend(paragraph.clone());
        if index + 1 < paragraphs.len() {
            airy.push(blank.clone());
        }
    }
    let tight: Vec<Line> = std::iter::once(title.clone())
        .chain(paragraphs.iter().flatten().cloned())
        .collect();
    let mut candidates = vec![airy, tight];
    for shown in (1..paragraphs.len()).rev() {
        let more = Line::from(Span::styled(
            format!("+{} more", paragraphs.len() - shown),
            theme.dim(),
        ));
        candidates.push(
            std::iter::once(title.clone())
                .chain(paragraphs[..shown].iter().flatten().cloned())
                .chain(std::iter::once(more))
                .collect(),
        );
    }
    // The last resort says how many there are and nothing else, which is what the prompt
    // line says at the widths that cannot hold a sentence either.
    candidates.push(vec![title]);

    let chrome = match bordered {
        true => 2,
        false => 0,
    };
    let Some(lines) = candidates
        .into_iter()
        .find(|lines| lines.len() + chrome <= body.height as usize)
    else {
        return;
    };
    let height = (lines.len() + chrome) as u16;
    let area = Rect::new(
        body.x + (body.width - width as u16) / 2,
        body.y + (body.height - height) / 2,
        width as u16,
        height,
    );
    frame.render_widget(Clear, area);
    let paragraph = Paragraph::new(lines);
    frame.render_widget(
        match bordered {
            true => paragraph.block(
                Block::bordered()
                    .border_style(Style::default().fg(theme.accent))
                    .padding(ratatui::widgets::Padding::horizontal(1)),
            ),
            false => paragraph,
        },
        area,
    );
}

/// Break `text` into lines of at most `width` characters, at spaces where there is one.
///
/// A word longer than the line is cut rather than left to overflow: git's words include
/// paths and refnames, and one of those is easily wider than a pane.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut length = 0;
    for word in text.split_whitespace() {
        let mut word = word.chars().collect::<Vec<char>>();
        // Longer than a line on its own: take what fits, and go round again with the rest.
        while word.len() > width {
            if length > 0 {
                lines.push(std::mem::take(&mut line));
                length = 0;
            }
            let head: String = word.drain(..width).collect();
            lines.push(head);
        }
        let word: String = word.into_iter().collect();
        let wanted = match length {
            0 => word.chars().count(),
            _ => word.chars().count() + 1,
        };
        if length + wanted > width && length > 0 {
            lines.push(std::mem::take(&mut line));
            length = 0;
        }
        if length > 0 {
            line.push(' ');
            length += 1;
        }
        length += word.chars().count();
        line.push_str(&word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use crate::domain::model::{CheckoutPath, RepoKey, WorkingTree};

    use super::*;
    use crate::ui::panes::PanesState;
    use crate::ui::render::fixtures::*;
    use crate::ui::render::panes::draw;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::Terminal;

    use crate::domain::model::{Refs, WorktreeNode};

    use crate::port::AgentStatus;

    #[test]
    fn the_panel_holds_what_the_prompt_line_had_to_cut() {
        // The line's job is to say something is wrong at any width; the panel's is to be
        // the place the whole of it can still be read without leaving the picker.
        let mut tree = tree();
        tree.repos[0].refs = Refs::Unreadable(TWO_REFS_REFUSAL.into());
        let mut state = PanesState::new(tree, None);

        let narrow = 60;
        let line = prompt_line(&state, narrow);
        assert!(
            !line.contains("refs/heads/chore/deps"),
            "the line cannot hold the second refname here: {line}"
        );

        press(&mut state, KeyCode::Char('!'));
        assert!(state.is_showing_conditions());
        insta::assert_snapshot!(screen(&state, 92, 18));
        let panel = screen(&state, narrow, 18);
        assert!(
            panel.contains("refs/heads/main") && panel.contains("refs/heads/chore/deps"),
            "both refnames are readable in the panel:\n{panel}"
        );
    }

    #[test]
    fn a_panel_too_short_for_everything_says_how_much_is_left() {
        // The same answer the line gives when it runs out of columns, for the same reason:
        // a screen that shows two of three things and looks complete is the failure mode.
        let mut tree = tree();
        tree.repos[0].refs = Refs::Unreadable(REFS_REFUSAL.into());
        tree.repos[1].refs = Refs::Unreadable(REFS_REFUSAL.into());
        let mut state = PanesState::new(tree, None);
        state.set_stale("herdr did not answer".into());
        assert_eq!(state.conditions().len(), 3);

        press(&mut state, KeyCode::Char('!'));
        // Below the prompt line, which carries a count of its own and would answer for the
        // panel's if the search were over the whole screen.
        let short = screen(&state, 92, 9);
        let panel = short.lines().skip(1).collect::<Vec<&str>>().join("\n");
        assert!(panel.contains("3 things are wrong"), "the count:\n{panel}");
        assert!(
            panel.contains("herdr did not answer"),
            "the one that fit, whole:\n{panel}"
        );
        assert!(panel.contains("+2 more"), "and what did not:\n{panel}");
    }

    #[test]
    fn the_key_that_reads_it_is_offered_only_while_there_is_something_to_read() {
        let state = PanesState::new(tree(), None);
        let hint = screen(&state, 92, 18);
        let hint = hint.lines().last().expect("the key hint").to_string();
        assert!(!hint.contains('!'), "nothing is wrong: {hint}");

        let mut tree = tree();
        tree.repos[0].refs = Refs::Unreadable(REFS_REFUSAL.into());
        let state = PanesState::new(tree, None);
        let hint = screen(&state, 92, 18);
        let hint = hint.lines().last().expect("the key hint").to_string();
        assert!(hint.contains("! what is wrong"), "and now: {hint}");
    }

    #[test]
    fn every_width_the_picker_supports_keeps_the_key_that_reads_the_rest() {
        // The narrow end is where it matters: that is where the line itself has given up
        // its words for a count, so the key is the only way back to them.
        let mut tree = tree();
        tree.repos[0].refs = Refs::Unreadable(TWO_REFS_REFUSAL.into());
        let state = PanesState::new(tree, None);
        for width in 24..=92 {
            let drawn = screen(&state, width, 18);
            let hint = drawn.lines().last().expect("the key hint").to_string();
            assert!(hint.contains('!'), "at {width}: {hint}");
        }
    }

    #[test]
    fn the_question_names_every_pane_that_stops() {
        // A finished worktree has panes in it, so this is the ordinary shape of the
        // question rather than an unusual one.
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
        // The path can be read from the breadcrumb behind the box; what is about to stop
        // cannot be read anywhere else.
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
        // afterwards. When nothing fits, drawing says so and the loop takes the question
        // back, or `y` is armed over a box nobody ever saw.
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
                    !draw(frame, &state, &theme()),
                    "too short for even the question"
                );
            })
            .unwrap();

        // And too narrow, which the height ladder cannot see.
        let mut narrow = Terminal::new(TestBackend::new(24, 40)).unwrap();
        narrow
            .draw(|frame| {
                assert!(
                    !draw(frame, &state, &theme()),
                    "`Delete this checkout` clipped out of `… and close 1 pane?` is a \
                         complete sentence and a false one"
                );
            })
            .unwrap();
    }

    #[test]
    fn a_pane_too_short_for_the_names_still_says_how_many_stop() {
        // The rung below the list, where the question itself takes over the number. It is
        // the same height a checkout with no panes gets, so there is no height at which `y`
        // is armed over a box that never said panes would close.
        let mut state = PanesState::new(tree(), None);
        state.set_working_trees(answers(&[("/wt/feat-login", WorkingTree::Clean)]));
        for _ in 0..2 {
            press(&mut state, KeyCode::Char('j'));
        }
        press(&mut state, KeyCode::Char('D'));
        insta::assert_snapshot!(screen(&state, 92, 9));
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
        // A pane too short for a box at all gets the key hint, which says the same thing.
        let mut state = PanesState::new(tree(), None);
        for _ in 0..3 {
            press(&mut state, KeyCode::Char('j'));
        }
        press(&mut state, KeyCode::Char('D'));
        insta::assert_snapshot!(screen(&state, 92, 10));
    }

    #[test]
    fn the_sweeps_question_lists_what_goes() {
        let mut state = sweeping_over(finished_tree(&["feat/login", "fix/crash", "chore/deps"]));
        ask(&mut state);
        insta::assert_snapshot!(screen(&state, 92, 16));
    }

    #[test]
    fn a_short_pane_lists_what_fits_and_counts_the_rest() {
        let mut state = sweeping_over(finished_tree(&[
            "feat/a", "feat/b", "feat/c", "feat/d", "feat/e", "feat/f", "feat/g", "feat/h",
        ]));
        ask(&mut state);
        insta::assert_snapshot!(screen(&state, 92, 12));
    }

    #[test]
    fn a_checkout_with_no_branch_changes_the_count_of_branches() {
        // Marked by hand — the sweep offers nothing it cannot judge — and its label is the
        // directory, which `git branch -d` is never given.
        let mut tree = finished_tree(&["feat/login", "fix/crash"]);
        tree.repos[0].worktrees.push(WorktreeNode {
            branch: None,
            checkout_path: "/wt/scratch".into(),
            is_primary: false,
            open_workspace_id: None,
            track: None,
            panes: vec![],
        });
        let mut state = sweeping_over(tree);
        for _ in 0..16 {
            if state.detail().contains("scratch") {
                break;
            }
            press(&mut state, KeyCode::Char('j'));
        }
        assert!(state.detail().contains("scratch"), "the cursor reached it");
        press(&mut state, KeyCode::Char(' '));
        ask(&mut state);
        insta::assert_snapshot!(screen(&state, 92, 14));
    }

    #[test]
    fn a_pane_too_small_for_the_sweeps_question_asks_none() {
        // The same floor as a deletion's question: two lines and a border, and a width the
        // title fits in.
        let mut state = sweeping_over(finished_tree(&["feat/login", "fix/crash"]));
        ask(&mut state);

        let mut short = Terminal::new(TestBackend::new(92, 7)).unwrap();
        short
            .draw(|frame| {
                assert!(
                    !draw(frame, &state, &theme()),
                    "too short for even the title"
                );
            })
            .unwrap();

        let mut narrow = Terminal::new(TestBackend::new(30, 40)).unwrap();
        narrow
            .draw(|frame| {
                assert!(
                    !draw(frame, &state, &theme()),
                    "`Delete 2 checkouts` clipped out of the title is a false sentence"
                );
            })
            .unwrap();
    }

    #[test]
    fn the_sweeps_question_counts_in_the_singular_and_says_when_no_branch_goes() {
        let mut tree = finished_tree(&["fix/crash", "feat/login"]);
        tree.repos[0].worktrees.push(WorktreeNode {
            branch: None,
            checkout_path: "/wt/scratch".into(),
            is_primary: false,
            open_workspace_id: None,
            track: None,
            panes: vec![],
        });
        let sweep = |paths: &[&str]| {
            let keys: Vec<_> = paths
                .iter()
                .map(|path| (RepoKey::of(&tree.repos[0]), CheckoutPath::for_test(path)))
                .collect();
            SweepRemoval::of(&tree, &keys)
        };
        assert_eq!(
            sweep_title(&sweep(&["/wt/fix-crash"])),
            "Delete 1 checkout and its branch?"
        );
        assert_eq!(sweep_title(&sweep(&["/wt/scratch"])), "Delete 1 checkout?");
        assert_eq!(
            sweep_title(&sweep(&["/wt/fix-crash", "/wt/scratch"])),
            "Delete 2 checkouts and 1 branch?"
        );
        assert_eq!(
            sweep_title(&sweep(&["/wt/fix-crash", "/wt/feat-login"])),
            "Delete 2 checkouts and their branches?"
        );
    }

    #[test]
    fn a_pane_too_small_to_lay_out_at_all_draws_no_question() {
        let mut state = sweeping_over(finished_tree(&["feat/login", "fix/crash"]));
        ask(&mut state);
        let mut tiny = Terminal::new(TestBackend::new(3, 3)).unwrap();
        tiny.draw(|frame| {
            assert!(
                !draw(frame, &state, &theme()),
                "nothing was drawn, so nothing may be answered"
            );
        })
        .unwrap();
    }

    #[test]
    fn a_pane_with_room_for_the_rows_but_not_the_air_lists_them_all() {
        let mut state = sweeping_over(finished_tree(&["feat/login", "fix/crash", "chore/deps"]));
        ask(&mut state);
        insta::assert_snapshot!(screen(&state, 92, 11));
    }

    #[test]
    fn a_pane_with_room_for_one_row_lists_it_and_counts_the_rest() {
        let mut state = sweeping_over(finished_tree(&["feat/login", "fix/crash", "chore/deps"]));
        ask(&mut state);
        insta::assert_snapshot!(screen(&state, 92, 10));
    }

    #[test]
    fn a_pane_with_room_for_the_title_alone_still_asks() {
        // The title carries the count, so the smallest box still says how many go.
        let mut state = sweeping_over(finished_tree(&["feat/login", "fix/crash", "chore/deps"]));
        ask(&mut state);
        let mut short = Terminal::new(TestBackend::new(92, 9)).unwrap();
        short
            .draw(|frame| assert!(draw(frame, &state, &theme())))
            .unwrap();
        insta::assert_snapshot!(screen(&state, 92, 9));
    }

    #[test]
    fn a_wide_pane_still_caps_the_box_at_eighty_columns() {
        // The path is what would turn the box into a banner, and it loses its middle instead.
        let mut tree = finished_tree(&["feat/login"]);
        tree.repos[0].worktrees[1].checkout_path =
            "/wt/a-directory-whose-name-runs-on-and-on-and-on-and-on-and-on-and-on/feat-login"
                .into();
        let mut state = sweeping_over(tree);
        ask(&mut state);
        insta::assert_snapshot!(screen(&state, 120, 14));
    }

    #[test]
    fn a_question_box_is_its_longest_line_and_what_the_box_spends() {
        // The longest line `the_sweeps_question_lists_what_goes` draws, and the box it
        // gets: the line plus what the box spends on itself.
        let wide = Rect::new(0, 0, 200, 40);
        assert_eq!(question_width(38, wide), 44);
    }

    #[test]
    fn a_question_box_is_never_wider_than_the_pane_or_eighty_columns() {
        assert_eq!(question_width(200, Rect::new(0, 0, 30, 40)), 30);
        assert_eq!(question_width(200, Rect::new(0, 0, 200, 40)), 80);
        assert_eq!(
            question_width(74, Rect::new(0, 0, 200, 40)),
            80,
            "80 exactly"
        );
        assert_eq!(
            question_width(73, Rect::new(0, 0, 200, 40)),
            79,
            "and just under"
        );
    }

    #[test]
    fn a_pane_one_column_short_of_the_title_and_the_box_asks_nothing() {
        // The box is its title and what the box spends, and a column less would clip the
        // title into a shorter sentence that is not the one being answered.
        let mut state = sweeping_over(finished_tree(&["feat/login", "fix/crash", "chore/deps"]));
        ask(&mut state);
        assert_eq!(
            question_width(38, Rect::new(0, 0, 44, 16)),
            44,
            "the title's own width"
        );

        for (width, asked) in [(44, true), (43, false)] {
            let mut terminal = Terminal::new(TestBackend::new(width, 16)).unwrap();
            terminal
                .draw(|frame| {
                    assert_eq!(draw(frame, &state, &theme()), asked, "at {width} columns");
                })
                .unwrap();
        }
    }

    #[test]
    fn a_pane_narrower_than_the_box_spends_asks_nothing_rather_than_panicking() {
        // The room a line has is the width less what the box spends, and in a pane this
        // narrow that is below zero long before the width floor turns the question away.
        let mut state = sweeping_over(finished_tree(&["feat/login"]));
        ask(&mut state);

        let mut terminal = Terminal::new(TestBackend::new(5, 16)).unwrap();
        terminal
            .draw(|frame| {
                assert!(!draw(frame, &state, &theme()));
            })
            .unwrap();
    }
}
