//! Where the cursor may stop, and how far one press of a key moves it.
//!
//! The list interleaves blank lines between groups and holds rows the cursor has no reason
//! to visit, so "one down" is not "one line down". Everything here is an index into the
//! display lines, and [`Row::is_selectable`] is the only thing that decides what is worth
//! going to.

use crate::domain::rows::*;

/// Index of the first line the cursor may sit on, at or after `from`, wrapping to the
/// start. `None` when nothing in the list is selectable at all.
pub fn next_row(rows: &[Row], lines: &[DisplayLine], from: usize) -> Option<usize> {
    let len = lines.len();
    (0..len).find_map(|offset| {
        let index = (from + offset) % len;
        selectable(rows, lines, index).then_some(index)
    })
}

/// Index of the first line the cursor may sit on, at or before `from`, wrapping to the end.
pub fn previous_row(rows: &[Row], lines: &[DisplayLine], from: usize) -> Option<usize> {
    let len = lines.len();
    (0..len).find_map(|offset| {
        let index = (from + len - offset) % len;
        selectable(rows, lines, index).then_some(index)
    })
}

/// The first line the cursor may sit on in the group `steps` away from the one holding
/// `from`, wrapping at both ends. `None` when there is no group to go to.
///
/// A group here is whatever heads a section of the list, which includes the panes that are
/// in no repository: on screen it is a heading with a blank line above it like any other,
/// and leaving it out would put the bottom of the list beyond the reach of these keys.
pub fn step_group(rows: &[Row], lines: &[DisplayLine], from: usize, steps: isize) -> Option<usize> {
    let groups = groups(rows, lines);
    if groups.is_empty() {
        return None;
    }
    // The group the cursor is in is the last one that starts at or before it.
    let current = groups
        .iter()
        .rposition(|(start, _)| *start <= from)
        .unwrap_or(0);
    let target = (current as isize + steps).rem_euclid(groups.len() as isize) as usize;
    Some(groups[target].1)
}

/// Where each group begins, and the first line inside it the cursor may sit on. A group with
/// nowhere to land is left out — it is not somewhere these keys can take you.
pub fn groups(rows: &[Row], lines: &[DisplayLine]) -> Vec<(usize, usize)> {
    let mut groups: Vec<(usize, Option<usize>)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let DisplayLine::Row(row) = line else {
            continue;
        };
        if rows[*row].reference.is_group() {
            groups.push((index, None));
        }
        if rows[*row].is_selectable() {
            if let Some((_, head)) = groups.last_mut() {
                head.get_or_insert(index);
            }
        }
    }
    groups
        .into_iter()
        .filter_map(|(start, head)| head.map(|head| (start, head)))
        .collect()
}

/// Whether the cursor may stop on this line.
pub fn selectable(rows: &[Row], lines: &[DisplayLine], index: usize) -> bool {
    match lines.get(index) {
        Some(DisplayLine::Row(row)) => rows[*row].is_selectable(),
        // A blank line separating two groups, or nothing there at all.
        Some(DisplayLine::Spacer) | None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::rows::fixtures::*;

    #[test]
    fn the_cursor_does_not_stop_on_a_checkout_that_is_going() {
        // A second Shift-D would race the first, and opening it would open something that is
        // being deleted underneath.
        let options = ViewOptions {
            removing: vec![CheckoutPath::for_test("/wt/fix-crash")],
            ..Default::default()
        };
        let rows = flatten(&tree(), &options);
        assert!(!find(&rows, "fix/crash").is_selectable());
        assert!(
            find(&rows, "fix/crash").is_idle,
            "it is still a checkout with nothing running in it"
        );
    }

    #[test]
    fn the_cursor_stops_only_where_there_is_somewhere_to_go() {
        let rows = flatten(&tree(), &ViewOptions::default());
        for row in &rows {
            let selectable = row.is_selectable();
            match row.reference {
                RowRef::Pane(..) | RowRef::Ungrouped(_) => {
                    assert!(selectable, "a pane is somewhere to go: {:?}", row.name)
                }
                RowRef::Worktree(..) => assert_eq!(
                    selectable, row.is_idle,
                    "only a checkout with nothing running: {:?}",
                    row.name
                ),
                RowRef::Repo(_) | RowRef::UngroupedRepo => {
                    assert!(!selectable, "a heading: {:?}", row.name)
                }
            }
        }
        // The fixture has all four kinds, so the loop above is not vacuous.
        assert!(rows.iter().any(|row| row.is_selectable()));
        assert!(rows.iter().any(|row| !row.is_selectable()));
    }

    #[test]
    fn every_repository_on_screen_has_something_selectable_under_it() {
        // Otherwise the arrow keys could reach a group and then have nowhere to go inside it.
        for options in [
            ViewOptions::default(),
            ViewOptions {
                query: "claude".into(),
                ..Default::default()
            },
            ViewOptions {
                state_filter: Some(StateFilter::Idle),
                ..Default::default()
            },
        ] {
            let rows = flatten(&tree(), &options);
            if rows.is_empty() {
                continue;
            }
            let mut group: Option<String> = None;
            let mut seen = false;
            for row in &rows {
                if row.reference.is_group() {
                    if let Some(label) = group {
                        assert!(seen, "{label:?} has nothing the cursor can land on");
                    }
                    group = row.name.clone();
                    seen = false;
                }
                seen |= row.is_selectable();
            }
            assert!(seen, "the last group has nothing the cursor can land on");
        }
    }

    #[test]
    fn cursor_movement_steps_over_everything_it_cannot_land_on_and_wraps() {
        let rows = flatten(&tree(), &ViewOptions::default());
        let lines = display_lines(&rows);
        let stops: Vec<usize> = (0..lines.len())
            .filter(|index| selectable(&rows, &lines, *index))
            .collect();

        // The first line is a repository heading, so the cursor starts below it.
        assert!(!selectable(&rows, &lines, 0));
        assert_eq!(next_row(&rows, &lines, 0), Some(stops[0]));
        assert_eq!(
            previous_row(&rows, &lines, 0),
            stops.last().copied(),
            "and wraps to the last thing there is to go to"
        );

        // A blank line, the heading after it, and a checkout with panes, in one go.
        let spacer = lines
            .iter()
            .position(|line| *line == DisplayLine::Spacer)
            .unwrap();
        assert_eq!(
            next_row(&rows, &lines, spacer),
            stops.iter().find(|stop| **stop > spacer).copied()
        );
        assert_eq!(
            previous_row(&rows, &lines, spacer),
            stops.iter().rev().find(|stop| **stop < spacer).copied()
        );

        // A line that is already a stop is where it stays.
        assert_eq!(next_row(&rows, &lines, stops[1]), Some(stops[1]));
        assert_eq!(previous_row(&rows, &lines, stops[1]), Some(stops[1]));
    }

    #[test]
    fn stepping_by_group_lands_on_the_first_thing_worth_going_to_in_it() {
        let rows = flatten(&tree(), &ViewOptions::default());
        let lines = display_lines(&rows);
        let name = |index: Option<usize>| match lines[index.unwrap()] {
            // A pane herdr tracks no agent in has no name; these tests are about where the
            // cursor lands, and the fixture's one such pane is the last thing it reaches.
            DisplayLine::Row(row) => rows[row].name.clone().unwrap_or_default(),
            DisplayLine::Spacer => panic!("a blank line is not somewhere to go"),
        };

        // From the first group's head, forward through every group and back round.
        let first = next_row(&rows, &lines, 0).unwrap();
        assert_eq!(name(Some(first)), "claude");
        let second = step_group(&rows, &lines, first, 1);
        assert_eq!(name(second), "claude", "me/site's first pane");
        let third = step_group(&rows, &lines, second.unwrap(), 1);
        assert_eq!(name(third), "", "the panes in no repository");
        assert_eq!(
            step_group(&rows, &lines, third.unwrap(), 1),
            Some(first),
            "and round to the start"
        );

        // Backwards from the first wraps to the last.
        assert_eq!(step_group(&rows, &lines, first, -1), third);
    }

    #[test]
    fn one_press_moves_exactly_one_group_wherever_in_it_the_cursor_was() {
        // The decision this pins: `←` never means "back to the head of the group I am in".
        // From anywhere inside a group it leaves it.
        let rows = flatten(&tree(), &ViewOptions::default());
        let lines = display_lines(&rows);

        let mut heads = Vec::new();
        let mut looking = false;
        for (index, line) in lines.iter().enumerate() {
            let DisplayLine::Row(row) = line else {
                continue;
            };
            looking |= rows[*row].reference.is_group();
            if looking && rows[*row].is_selectable() {
                heads.push(index);
                looking = false;
            }
        }
        assert_eq!(heads.len(), 3, "two repositories and the panes in neither");

        // Which group each line belongs to: how many group rows have gone past.
        let mut group_of = vec![0usize; lines.len()];
        let mut group = 0;
        for (index, line) in lines.iter().enumerate() {
            if let DisplayLine::Row(row) = line {
                if rows[*row].reference.is_group() && index > 0 {
                    group += 1;
                }
            }
            group_of[index] = group;
        }

        let mut checked = 0;
        for (index, here) in group_of.iter().enumerate() {
            if !selectable(&rows, &lines, index) {
                continue;
            }
            let here = *here as isize;
            let head = |steps: isize| {
                Some(heads[(here + steps).rem_euclid(heads.len() as isize) as usize])
            };
            assert_eq!(
                step_group(&rows, &lines, index, 1),
                head(1),
                "\u{2192} from {index}"
            );
            assert_eq!(
                step_group(&rows, &lines, index, -1),
                head(-1),
                "\u{2190} from {index}"
            );
            checked += 1;
        }
        assert!(
            checked > heads.len(),
            "some groups hold more than the head they are entered at"
        );
    }

    #[test]
    fn there_is_nowhere_to_step_in_an_empty_list() {
        assert_eq!(step_group(&[], &[], 0, 1), None);
        assert_eq!(step_group(&[], &[], 0, -1), None);
    }

    #[test]
    fn cursor_movement_on_an_empty_list_has_no_answer_rather_than_panicking() {
        assert_eq!(next_row(&[], &[], 0), None);
        assert_eq!(previous_row(&[], &[], 0), None);
    }
}
