//! The words the picker puts on the screen for a value `domain` handed it.
//!
//! `domain` answers what is true — which destinations there are, what step opening a branch
//! is on, what order the list is in, what the picker cannot do and why — and every one of
//! those answers is a value rather than a sentence. Turning one into the sentence a reader
//! sees is this module's job, so that a change to the wording is a change here and a change
//! to the facts is a change there.
//!
//! What belongs here is the whole picker's vocabulary rather than one view's: the same
//! destination is named on a row, in a preview caption and in the step that lands a pane in
//! it, and those three reading differently would be a bug nothing would catch.

use crate::domain::dest::{Destination, Landing, SpaceName, TabName};
use crate::domain::notice::Condition;
use crate::domain::order::{Order, SortKey};
use crate::domain::preview::Refusal;
use crate::domain::progress::Stage;

/// Shown for a pane herdr is not tracking an agent in.
pub const UNNAMED_PANE: &str = "shell";

/// A workspace: its id, and its name where it has one.
///
/// The id is first because it is what herdr's own navigator shows, so it is what the user is
/// matching against; the name is what says which project it is.
pub fn space(space: &SpaceName) -> String {
    match &space.label {
        Some(label) => format!("{}  {label}", space.workspace_id),
        None => space.workspace_id.clone(),
    }
}

/// A tab, under the space it is in. A tab with no name of its own is named by its id, which
/// is the only thing that tells two of them apart.
pub fn tab(tab: &TabName) -> String {
    let space = space(&tab.space);
    match &tab.label {
        Some(label) => format!("{space} / {label}"),
        None => format!("{space} / {}", tab.tab_id),
    }
}

/// Where the pane would land, as the preview captions it.
pub fn landing(at: &Landing) -> String {
    match at {
        Landing::Tab(name) => tab(name),
        Landing::NewTabIn(name) => format!("{} \u{2014} a new tab", space(name)),
        Landing::NewSpace => "a space of its own \u{2014} a new space".to_string(),
    }
}

/// What a destination row reads as.
pub fn destination(destination: &Destination) -> String {
    match destination {
        Destination::SplitHere { direction, .. } => format!("split {}", direction.as_str()),
        Destination::ExistingTab { tab: name, zoomed } => {
            let label = tab(name);
            if *zoomed {
                format!("{label}  (zoomed)")
            } else {
                label
            }
        }
        Destination::ExistingSpace { space: name } => {
            format!("{} \u{2192} new tab", space(name))
        }
        // The group column already says "new space"; this says what happens to it.
        Destination::NewSpace => "on its own".to_string(),
    }
}

/// A heading for the group a destination row belongs to, when it starts one.
pub fn destination_group(destination: &Destination) -> &'static str {
    match destination {
        Destination::SplitHere { .. } => "here",
        Destination::ExistingTab { .. } => "existing tab",
        Destination::ExistingSpace { .. } => "existing space",
        // Its own group, so it does not read as one more "existing space".
        Destination::NewSpace => "new space",
    }
}

/// Why the destination under the cursor cannot take the pane.
pub fn refusal(refusal: Refusal) -> String {
    match refusal {
        Refusal::Zoomed => "this tab is zoomed, and herdr will not move a pane into a zoomed \
                            tab. Unzoom it first, or pick somewhere else."
            .to_string(),
    }
}

/// What the picker announces while a branch is being opened.
pub fn stage(stage: &Stage) -> String {
    match stage {
        Stage::Starting { branch } => format!("opening {branch}"),
        Stage::Fetching { remote, branch } => format!("fetching {remote}/{branch}"),
        Stage::Creating { branch } => format!("creating the worktree for {branch}"),
        Stage::Opening { branch } => format!("opening the checkout for {branch}"),
        Stage::Landing { destination } => match destination {
            Destination::SplitHere { .. } => {
                "moving the pane beside the one you came from".to_string()
            }
            Destination::ExistingTab { tab: name, .. } => {
                format!("moving the pane into {}", tab(name))
            }
            Destination::ExistingSpace { space: name } => {
                format!("moving the pane into {}", space(name))
            }
            // herdr already put it in a space of its own; there is nothing to move.
            Destination::NewSpace => "focusing the new pane".to_string(),
        },
    }
}

/// What the branch list is ordered by.
pub fn sort_key(key: SortKey) -> &'static str {
    match key {
        SortKey::State => "state",
        SortKey::Updated => "updated",
        SortKey::Name => "name",
    }
}

/// The order, with the arrow that says which way it runs.
pub fn order(order: Order) -> String {
    let arrow = if order.descending() {
        '\u{2193}'
    } else {
        '\u{2191}'
    };
    format!("{} {arrow}", sort_key(order.key))
}

/// One condition, as the prompt line and the panel both read it.
pub fn condition(condition: &Condition) -> String {
    match condition {
        Condition::Stale(words) => words.clone(),
        Condition::RefsUnreadable { repo, words } => format!("{repo}: refs unreadable: {words}"),
        Condition::SweepTrouble(words) => words.clone(),
    }
}

/// The one-line form: the first condition, and a count of what it is standing in for.
///
/// One line can hold one of these, so the rest are counted rather than dropped — a line that
/// showed the first and said nothing about the others would read as the whole story. The
/// count is what the reader follows to the unabridged list.
pub fn conditions_line(conditions: &[Condition]) -> Option<String> {
    let first = conditions.first()?;
    Some(match conditions.len() - 1 {
        0 => condition(first),
        more => format!("{} (+{more} more)", condition(first)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dest::fixtures::{space_name, tab_name};
    use crate::port::SplitDirection;

    #[test]
    fn a_space_is_named_by_its_id_first_because_that_is_what_herdr_shows() {
        assert_eq!(space(&space_name("w1", "app")), "w1  app");
        assert_eq!(space(&space_name("w2", "")), "w2");
    }

    #[test]
    fn a_tab_with_no_name_of_its_own_falls_back_to_the_id_that_tells_it_apart() {
        assert_eq!(
            tab(&tab_name("w1", "app", "w1:t1", "agents")),
            "w1  app / agents"
        );
        assert_eq!(tab(&tab_name("w1", "app", "w1:t2", "")), "w1  app / w1:t2");
        assert_eq!(tab(&tab_name("w2", "", "w2:t1", "logs")), "w2 / logs");
    }

    #[test]
    fn a_destination_row_says_where_and_a_zoomed_one_says_so_in_the_same_line() {
        assert_eq!(
            destination(&Destination::SplitHere {
                tab_id: "w1:t1".into(),
                target_pane_id: "w1:p1".into(),
                direction: SplitDirection::Right,
            }),
            "split right"
        );
        assert_eq!(
            destination(&Destination::ExistingTab {
                tab: tab_name("w2", "", "w2:t1", "logs"),
                zoomed: false,
            }),
            "w2 / logs"
        );
        assert_eq!(
            destination(&Destination::ExistingTab {
                tab: tab_name("w2", "", "w2:t1", "logs"),
                zoomed: true,
            }),
            "w2 / logs  (zoomed)",
            "the row stays in the list and carries its own reason"
        );
        assert_eq!(
            destination(&Destination::ExistingSpace {
                space: space_name("w1", "app"),
            }),
            "w1  app \u{2192} new tab"
        );
        assert_eq!(destination(&Destination::NewSpace), "on its own");
    }

    #[test]
    fn a_landing_reads_as_the_caption_over_the_preview() {
        assert_eq!(
            landing(&Landing::Tab(tab_name("w1", "app", "w1:t1", "agents"))),
            "w1  app / agents"
        );
        assert_eq!(
            landing(&Landing::NewTabIn(space_name("w2", "notes"))),
            "w2  notes \u{2014} a new tab"
        );
        assert_eq!(
            landing(&Landing::NewSpace),
            "a space of its own \u{2014} a new space"
        );
    }

    #[test]
    fn a_blocked_destination_says_what_to_do_about_it() {
        let words = refusal(Refusal::Zoomed);
        assert!(words.contains("zoomed"), "got {words}");
        assert!(
            words.contains("Unzoom"),
            "a reason with no way out is half an answer: {words}"
        );
    }

    #[test]
    fn every_stage_says_what_it_is_doing_and_to_what() {
        assert_eq!(
            stage(&Stage::Starting {
                branch: "feat/login".into()
            }),
            "opening feat/login"
        );
        assert_eq!(
            stage(&Stage::Fetching {
                remote: "origin".into(),
                branch: "feat/login".into()
            }),
            "fetching origin/feat/login"
        );
        assert_eq!(
            stage(&Stage::Creating {
                branch: "feat/login".into()
            }),
            "creating the worktree for feat/login"
        );
        assert_eq!(
            stage(&Stage::Opening {
                branch: "fix/crash".into()
            }),
            "opening the checkout for fix/crash"
        );
    }

    #[test]
    fn landing_reads_as_what_happens_to_the_pane() {
        assert_eq!(
            stage(&Stage::Landing {
                destination: Destination::SplitHere {
                    tab_id: "w1:t1".into(),
                    target_pane_id: "w1:p1".into(),
                    direction: SplitDirection::Right,
                }
            }),
            "moving the pane beside the one you came from"
        );
        assert_eq!(
            stage(&Stage::Landing {
                destination: Destination::ExistingTab {
                    tab: tab_name("w1", "app", "w1:t2", "logs"),
                    zoomed: false,
                }
            }),
            "moving the pane into w1  app / logs"
        );
        assert_eq!(
            stage(&Stage::Landing {
                destination: Destination::NewSpace
            }),
            "focusing the new pane",
            "nothing is moved: herdr already put it in a space of its own"
        );
    }

    #[test]
    fn the_arrow_describes_the_values_rather_than_the_rows() {
        assert_eq!(order(Order::default()), "state \u{2193}");
        assert_eq!(order(Order::default().cycle()), "updated \u{2193}");
        assert_eq!(
            order(Order::default().cycle().reverse()),
            "updated \u{2191}"
        );
        // a to z is ascending, so the name key points the other way to begin with.
        assert_eq!(order(Order::default().cycle().cycle()), "name \u{2191}");
        assert_eq!(
            order(Order::default().cycle().cycle().reverse()),
            "name \u{2193}"
        );
    }

    #[test]
    fn a_condition_says_which_repository_could_not_be_read() {
        assert_eq!(
            condition(&Condition::RefsUnreadable {
                repo: "me/app".into(),
                words: "fatal: bad ref".into(),
            }),
            "me/app: refs unreadable: fatal: bad ref"
        );
        assert_eq!(
            condition(&Condition::SweepTrouble(
                "me/app: gh could not be run".into()
            )),
            "me/app: gh could not be run",
            "what gh said already names its repository"
        );
    }

    #[test]
    fn nothing_wrong_puts_nothing_on_the_line() {
        assert_eq!(conditions_line(&[]), None);
    }

    #[test]
    fn a_line_that_can_hold_one_says_how_many_it_is_standing_in_for() {
        let conditions = [
            Condition::RefsUnreadable {
                repo: "me/app".into(),
                words: "fatal: bad ref".into(),
            },
            Condition::RefsUnreadable {
                repo: "me/docs".into(),
                words: "fatal: index file corrupt".into(),
            },
            Condition::SweepTrouble("me/app: gh could not be run".into()),
        ];
        assert_eq!(
            conditions_line(&conditions).as_deref(),
            Some("me/app: refs unreadable: fatal: bad ref (+2 more)"),
            "the count covers everything unsaid, gh included: a reader who is told about one \
             of three and nothing about the other two has been told the wrong thing"
        );
        assert_eq!(
            conditions_line(&conditions[..1]).as_deref(),
            Some("me/app: refs unreadable: fatal: bad ref"),
            "the only one there is stands in for nothing"
        );
    }
}
