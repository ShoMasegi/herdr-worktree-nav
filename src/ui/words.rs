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
use crate::domain::model::WorkingTree;
use crate::domain::model::{Position, Tree};
use crate::domain::notice::Condition;
use crate::domain::order::{Order, SortKey};
use crate::domain::preview::Refusal;
use crate::domain::progress::Stage;
use crate::domain::rows::{Row, RowRef, StateFilter};
use crate::domain::sweep::{Half, Mark, Reason, Refusal as SweepRefusal};
use crate::port::{AgentStatus, PullRequestOutcome, Track};

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

/// A checkout holding uncommitted changes or untracked files, with the gap that precedes it.
const DIRTY: &str = "  \u{2731}";
/// A checkout git would not answer for. The same width as [`DIRTY`] and mutually exclusive
/// with it — a working tree is dirty, clean, or unread — so it costs the row nothing extra.
const UNREADABLE: &str = "  ?";
/// git cannot find the ref this branch tracks. A word rather than a glyph: it is the one of
/// these that says a branch is finished with, and it is worth being unmissable.
const GONE: &str = "gone";

/// What a row is called, which for a group is its name and how much it holds.
///
/// The count rides with the name rather than in the meta column: a group's meta column is
/// left to the checkout directly beneath it, and a heading that did not say how much was
/// under it would be a heading worth folding and this picker does not fold.
pub fn row_label(row: &Row) -> String {
    match (&row.name, row.reference) {
        (Some(name), RowRef::Repo(_)) => format!("{name} ({})", row.panes),
        (Some(name), _) => name.clone(),
        // The group of panes no repository holds. It is named by what it is, because there
        // is nothing else to call it.
        (None, RowRef::UngroupedRepo) => format!("not in any repository ({})", row.panes),
        (None, _) => UNNAMED_PANE.to_string(),
    }
}

/// What a row says about itself between its label and its `no pane` note.
///
/// Two spaces before each, the same gap the note uses. Everything is optional and most rows
/// have none of it, which is why these ride beside the label rather than in a column of
/// their own: a column that is blank on most rows is a permanent gap between the name and
/// the path.
///
/// Dirty comes first because it is the one that stops a checkout being removable.
pub fn marks(row: &Row) -> String {
    let mut out = String::new();
    match row.working_tree {
        Some(WorkingTree::Dirty) => out.push_str(DIRTY),
        Some(WorkingTree::Unreadable) => out.push_str(UNREADABLE),
        // Clean and not-yet-answered both draw nothing, for different reasons: one has
        // nothing to report and the other has nothing to report *yet*.
        Some(WorkingTree::Clean) | None => {}
    }
    out.push_str(&track_mark(row.position));
    out
}

/// How much room a row's marks are allowed to take without moving the meta column.
///
/// The marker for what a `git status` said is counted whether it is showing or not — and
/// `✱` and `?` are the same width, so one reserve serves both. It arrives a beat after the
/// first frame, with the list already on screen, and the meta column is a maximum over every
/// row: measuring only what is showing would jump every path in the list sideways once,
/// including the paths of rows in repositories that have not changed at all.
///
/// Ahead, behind and `gone` are measured exactly, because they are known before the first
/// frame and cannot change without a reload — which redraws the whole list anyway.
pub fn marks_reserve(row: &Row) -> usize {
    if !row.reference.is_worktree() {
        return 0;
    }
    DIRTY.chars().count() + track_mark(row.position).chars().count()
}

/// Where the branch stands against its upstream, with the gap that precedes it.
pub fn track_mark(position: Position) -> String {
    match position {
        Position::Said(Some(Track::Gone)) => format!("  {GONE}"),
        Position::Said(Some(Track::Ahead(ahead))) => format!("  \u{2191}{ahead}"),
        Position::Said(Some(Track::Behind(behind))) => format!("  \u{2193}{behind}"),
        Position::Said(Some(Track::Diverged { ahead, behind })) => {
            format!("  \u{2191}{ahead}\u{2193}{behind}")
        }
        // Nothing to draw, for three reasons the row has no room to tell apart. git
        // reported nothing, which is not this row's to interpret; a field git printed could
        // not be read, where there is no position to draw at all and a marker that is wrong
        // is worse than none — kind 4 of `docs/en/error-handling.md`; and more than one ref
        // names this checkout, where the domain declined to choose. `app::dump` is where
        // the three are told apart, and in a sweep the third is on the row as `refs
        // disagree`, because it is the half the sweep needed.
        Position::Said(Some(Track::Unreadable) | None)
        | Position::Contested
        | Position::NotSaid => String::new(),
    }
}

/// The same marks with no gap in front, for a page that is not a row of the list.
pub fn track_alone(track: Track) -> String {
    track_mark(Position::Said(Some(track)))
        .trim_start()
        .to_string()
}

/// Narrowing the list to one agent state, as the chip beside the search box reads.
pub fn state_filter(filter: StateFilter) -> &'static str {
    match filter {
        StateFilter::Blocked => "blocked",
        StateFilter::Working => "working",
        StateFilter::Idle => "idle",
        StateFilter::Done => "done",
    }
}

/// herdr's wording for an agent state. `None` when there is no agent to describe.
pub fn status(status: AgentStatus) -> Option<&'static str> {
    match status {
        AgentStatus::Blocked => Some("blocked"),
        AgentStatus::Working => Some("working"),
        AgentStatus::Done => Some("done"),
        AgentStatus::Idle => Some("idle"),
        AgentStatus::Unknown => None,
    }
}

/// Show a path under the user's home as `~/...`. Popups are narrower than a full pane, and
/// sixteen characters of `/Users/someone` are the least useful part of a checkout path.
pub fn abbreviate(path: &str, home: Option<&str>) -> String {
    let Some(home) = home.filter(|home| !home.is_empty()) else {
        return path.to_string();
    };
    let home = home.trim_end_matches('/');
    match path.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// The breadcrumb shown under the list for the row the cursor is on.
///
/// This is where the checkout path lives. The navigator keeps its rows to a label and one
/// meta column and puts the fuller context here, so the list stays scannable.
pub fn detail(tree: &Tree, reference: RowRef) -> String {
    let parts: Vec<String> = match reference {
        RowRef::Repo(repo_index) => {
            let Some(repo) = tree.repos.get(repo_index) else {
                return String::new();
            };
            let panes: usize = repo.worktrees.iter().map(|w| w.panes.len()).sum();
            let worktrees = repo.worktrees.len();
            vec![
                repo.display_name.clone(),
                format!("{worktrees} {}", plural(worktrees, "worktree")),
                format!("{panes} {}", plural(panes, "pane")),
                repo.repo_root.clone(),
            ]
        }
        RowRef::Worktree(repo_index, worktree_index) => {
            let Some(repo) = tree.repos.get(repo_index) else {
                return String::new();
            };
            let Some(worktree) = repo.worktrees.get(worktree_index) else {
                return String::new();
            };
            let mut parts = vec![repo.display_name.clone(), worktree.label().to_string()];
            if worktree.is_primary {
                parts.push("main checkout".to_string());
            }
            parts.push(worktree.checkout_path.clone());
            parts
        }
        RowRef::Pane(repo_index, worktree_index, pane_index) => {
            let Some(repo) = tree.repos.get(repo_index) else {
                return String::new();
            };
            let Some(worktree) = repo.worktrees.get(worktree_index) else {
                return String::new();
            };
            let Some(pane) = worktree.panes.get(pane_index) else {
                return String::new();
            };
            let mut parts = vec![
                repo.display_name.clone(),
                worktree.label().to_string(),
                pane.pane_id.clone(),
            ];
            if let Some(label) = status(pane.agent_status) {
                parts.push(label.to_string());
            }
            parts.push(worktree.checkout_path.clone());
            parts
        }
        RowRef::UngroupedRepo => vec![
            "not inside any git work tree".to_string(),
            format!(
                "{} {}",
                tree.ungrouped.len(),
                plural(tree.ungrouped.len(), "pane")
            ),
        ],
        RowRef::Ungrouped(index) => {
            let Some(pane) = tree.ungrouped.get(index) else {
                return String::new();
            };
            let mut parts = vec![
                pane.display_name
                    .clone()
                    .unwrap_or_else(|| UNNAMED_PANE.to_string()),
                pane.pane_id.clone(),
            ];
            if let Some(label) = status(pane.agent_status) {
                parts.push(label.to_string());
            }
            parts
        }
    };
    parts.join(" \u{b7} ")
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    }
}

/// What a sweep's row says beside its mark, or nothing.
///
/// [`Reason::Gone`] is left out: [`marks`] draws it as the branch's upstream marker on every
/// row it is true of, and `judge` offers that reason only where `track` is `Gone`, so
/// repeating it puts the same word on the row twice. (The converse does not hold: a row
/// whose track is gone is not offered while it is primary, running, being removed, or not
/// known to be clean.)
///
/// A refusal is left out too: the absence of a box says it, and `Space` answers on the
/// prompt line — see [`sweep_refusal`].
pub fn sweep_note(mark: &Mark) -> Option<String> {
    match mark {
        Mark::Going(Reason::PullRequest { number, outcome }) => {
            let what = match outcome {
                PullRequestOutcome::Merged => "merged",
                PullRequestOutcome::Closed => "closed",
            };
            Some(format!("PR #{number} {what}"))
        }
        Mark::Unjudged(half) | Mark::GoingUnjudged(half) => Some(unjudged(*half).to_string()),
        Mark::Going(Reason::Gone) | Mark::GoingByHand | Mark::Staying | Mark::Refused(_) => None,
    }
}

/// Which half of the sweep's question nobody could answer, as the row says it.
fn unjudged(half: Half) -> &'static str {
    match half {
        Half::Refs => "refs unreadable",
        Half::RefsDisagree => "refs disagree",
        Half::PullRequests => "PR unknown",
    }
}

/// Why `Space` did nothing on this row, for the prompt line.
///
/// On demand rather than on the row: the answer is only wanted by somebody who has just
/// tried, and these are sentences rather than labels.
pub fn sweep_refusal(mark: &Mark) -> Option<&'static str> {
    Some(match mark.refused()? {
        SweepRefusal::Primary => "the repository itself",
        SweepRefusal::Running => "panes are running in it",
        SweepRefusal::Removing => "already being removed",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dest::fixtures::{space_name, tab_name};
    use crate::domain::model::{CheckoutPath, WorkingTree};
    use crate::domain::rows::fixtures::*;
    use crate::domain::rows::{flatten, ViewOptions};
    use crate::port::SplitDirection;
    use std::collections::BTreeMap;
    use std::num::NonZeroU32;

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

    /// The answers map with one checkout given an answer, which is all these need.
    fn answered(working_tree: WorkingTree) -> BTreeMap<CheckoutPath, WorkingTree> {
        BTreeMap::from([(CheckoutPath::for_test("/wt/fix-crash"), working_tree)])
    }

    /// `None` is a checkout nobody has answered for, which is a third thing and not a
    /// synonym for clean. A bool here would collapse the two into one `false`, naming
    /// neither state.
    fn marks_for(working_tree: Option<WorkingTree>, position: Position) -> String {
        let mut tree = tree();
        tree.repos[0].worktrees[2].position = position;
        let options = ViewOptions {
            working_trees: working_tree.map(answered).unwrap_or_default(),
            ..Default::default()
        };
        marks(find(&flatten(&tree, &options), "fix/crash"))
    }

    #[test]
    fn a_checkout_with_nothing_to_report_says_nothing() {
        assert_eq!(
            marks_for(None, Position::Said(None)),
            "",
            "nobody has answered for it"
        );
        assert_eq!(
            marks_for(Some(WorkingTree::Clean), Position::Said(None)),
            "",
            "and git answered and had nothing to report — the same absence of a marker for \
             two different reasons, which is the whole of why the list is not rebuilt when \
             one becomes the other"
        );
    }

    #[test]
    fn every_answer_a_working_tree_can_give_reads_on_its_own() {
        assert_eq!(
            marks_for(Some(WorkingTree::Dirty), Position::Said(None)),
            "  ✱"
        );
        assert_eq!(
            marks_for(Some(WorkingTree::Unreadable), Position::Said(None)),
            "  ?"
        );
    }

    #[test]
    fn the_room_kept_for_the_marks_does_not_depend_on_the_dirty_answer() {
        // Which is what stops every path in the list moving sideways when a `git status`
        // finally answers.
        let mut tree = tree();
        tree.repos[0].worktrees[2].position = Position::Said(Some(Track::Gone));
        let clean = flatten(&tree, &ViewOptions::default());
        let dirty = flatten(
            &tree,
            &ViewOptions {
                working_trees: answered(WorkingTree::Dirty),
                ..Default::default()
            },
        );
        assert_eq!(
            marks_reserve(find(&clean, "fix/crash")),
            marks_reserve(find(&dirty, "fix/crash"))
        );
        assert_eq!(
            marks_reserve(find(&clean, "me/app")),
            0,
            "a repository heading has no checkout to say anything about"
        );
    }

    #[test]
    fn each_thing_a_checkout_can_be_reads_on_its_own() {
        assert_eq!(
            marks_for(Some(WorkingTree::Dirty), Position::Said(None)),
            "  \u{2731}"
        );
        assert_eq!(
            marks_for(
                None,
                Position::Said(Some(Track::Ahead(NonZeroU32::new(2).unwrap())))
            ),
            "  \u{2191}2"
        );
        assert_eq!(
            marks_for(
                None,
                Position::Said(Some(Track::Behind(NonZeroU32::new(1).unwrap())))
            ),
            "  \u{2193}1"
        );
        assert_eq!(
            marks_for(
                None,
                Position::Said(Some(Track::Diverged {
                    ahead: NonZeroU32::new(2).unwrap(),
                    behind: NonZeroU32::new(1).unwrap()
                }))
            ),
            "  \u{2191}2\u{2193}1",
            "one gap, not two: they are one answer"
        );
        assert_eq!(marks_for(None, Position::Said(Some(Track::Gone))), "  gone");
        // Three that draw what a row with nothing to report draws: a position that went
        // unread, more than one ref naming this checkout, and no ref naming it. They are
        // told apart on `app::dump`'s page, not here.
        assert_eq!(marks_for(None, Position::Said(Some(Track::Unreadable))), "");
        assert_eq!(marks_for(None, Position::Contested), "");
        assert_eq!(marks_for(None, Position::NotSaid), "");
        assert_eq!(marks_for(None, Position::Said(None)), "");
    }

    #[test]
    fn a_dirty_checkout_whose_upstream_is_gone_says_both() {
        // Which is the pair that decides whether a checkout can be swept: gone says it is
        // finished with, and dirty says it cannot go anyway.
        assert_eq!(
            marks_for(Some(WorkingTree::Dirty), Position::Said(Some(Track::Gone))),
            "  \u{2731}  gone"
        );
    }

    #[test]
    fn a_checkout_holding_uncommitted_work_is_marked_as_such() {
        let options = ViewOptions {
            working_trees: answered(WorkingTree::Dirty),
            ..Default::default()
        };
        let rows = flatten(&tree(), &options);
        assert_eq!(
            find(&rows, "fix/crash").working_tree,
            Some(WorkingTree::Dirty)
        );
        assert_eq!(
            find(&rows, "feat/login").working_tree,
            None,
            "nobody asked about that one"
        );
    }

    #[test]
    fn a_checkout_whose_answer_has_not_arrived_is_drawn_without_a_marker() {
        // Asking whether a checkout is dirty is a process per checkout, so the answers arrive
        // after the first frame. The alternative is a marker that is wrong for a moment.
        let rows = flatten(&tree(), &ViewOptions::default());
        assert_eq!(find(&rows, "fix/crash").working_tree, None);
        assert_eq!(marks(find(&rows, "fix/crash")), "", "and so draws nothing");
    }

    #[test]
    fn a_checkout_carries_what_git_said_about_its_branch() {
        let mut tree = tree();
        tree.repos[0].worktrees[2].position = Position::Said(Some(Track::Gone));
        let rows = flatten(&tree, &ViewOptions::default());
        assert_eq!(
            find(&rows, "fix/crash").position,
            Position::Said(Some(Track::Gone))
        );
        assert_eq!(find(&rows, "feat/login").position, Position::NotSaid);
        assert_eq!(
            find(&rows, "me/app").position,
            Position::NotSaid,
            "a repository heading has no branch of its own"
        );
    }

    #[test]
    fn paths_under_the_home_directory_are_shortened() {
        assert_eq!(
            abbreviate("/home/me/Workspace/app", Some("/home/me")),
            "~/Workspace/app"
        );
        assert_eq!(
            abbreviate("/home/me", Some("/home/me")),
            "~",
            "the home directory itself is the whole of it"
        );
        assert_eq!(
            abbreviate("/home/me/src", Some("/home/me/")),
            "~/src",
            "a trailing slash on the home does not leave a doubled one behind"
        );
    }

    #[test]
    fn a_path_outside_the_home_directory_is_left_alone() {
        assert_eq!(abbreviate("/srv/app", Some("/home/me")), "/srv/app");
        // A sibling that merely shares the prefix must not be mangled.
        assert_eq!(
            abbreviate("/home/median/app", Some("/home/me")),
            "/home/median/app"
        );
        assert_eq!(abbreviate("/home/me", Some("/home/me")), "~");
        assert_eq!(abbreviate("/home/me/x", Some("/home/me/")), "~/x");
        assert_eq!(abbreviate("/home/me/x", None), "/home/me/x");
        assert_eq!(abbreviate("/home/me/x", Some("")), "/home/me/x");
    }

    #[test]
    fn the_breadcrumb_carries_the_checkout_path_the_rows_no_longer_show() {
        let tree = tree();
        assert_eq!(
            detail(&tree, RowRef::Worktree(0, 1)),
            "me/app · feat/login · /wt/feat-login"
        );
        assert_eq!(
            detail(&tree, RowRef::Worktree(0, 0)),
            "me/app · main · main checkout · /wt/main"
        );
        assert_eq!(
            detail(&tree, RowRef::Pane(0, 0, 0)),
            "me/app · main · w1:p1 · working · /wt/main"
        );
        assert_eq!(
            detail(&tree, RowRef::Repo(0)),
            "me/app · 3 worktrees · 3 panes · /src/app"
        );
    }

    #[test]
    fn the_breadcrumb_is_empty_rather_than_panicking_on_a_stale_reference() {
        let tree = tree();
        assert_eq!(detail(&tree, RowRef::Repo(99)), "");
        assert_eq!(detail(&tree, RowRef::Worktree(0, 99)), "");
        assert_eq!(detail(&tree, RowRef::Pane(0, 0, 99)), "");
        assert_eq!(detail(&tree, RowRef::Ungrouped(99)), "");
    }

    #[test]
    fn a_row_a_sweep_has_something_to_say_about_says_it_beside_its_mark() {
        assert_eq!(
            sweep_note(&Mark::Going(Reason::PullRequest {
                number: 123,
                outcome: PullRequestOutcome::Merged,
            }))
            .as_deref(),
            Some("PR #123 merged"),
            "the number is what makes the reason checkable"
        );
        assert_eq!(
            sweep_note(&Mark::Going(Reason::PullRequest {
                number: 4,
                outcome: PullRequestOutcome::Closed,
            }))
            .as_deref(),
            Some("PR #4 closed"),
            "merged says the work is in and closed says it was abandoned, and the wrong \
             way round tells someone their work landed as they delete the only copy"
        );
        assert_eq!(
            sweep_note(&Mark::Unjudged(Half::PullRequests)).as_deref(),
            Some("PR unknown"),
            "a row gh could not judge says so rather than looking like one with nothing \
             to find"
        );
        assert_eq!(
            sweep_note(&Mark::GoingUnjudged(Half::Refs)).as_deref(),
            Some("refs unreadable"),
            "marked by hand, it still says nobody judged it — and which half"
        );
        assert_eq!(
            sweep_note(&Mark::Unjudged(Half::RefsDisagree)).as_deref(),
            Some("refs disagree"),
            "a walk that worked and named two branches at one path is not a walk that              failed, and the fix for it is not the same one"
        );
    }

    #[test]
    fn a_row_with_nothing_of_the_sweeps_to_show_shows_nothing() {
        assert_eq!(
            sweep_note(&Mark::Going(Reason::Gone)),
            None,
            "its reason is the upstream marker the row already draws, and saying it again \
             would put the same word on the row twice"
        );
        assert_eq!(
            sweep_note(&Mark::GoingByHand),
            None,
            "a note there would make the sweep look as though it had agreed"
        );
        assert_eq!(sweep_note(&Mark::Staying), None);
        assert_eq!(
            sweep_note(&Mark::Refused(SweepRefusal::Primary)),
            None,
            "a refusal is said by the absence of a box, not by a sentence where the label \
             goes"
        );
    }

    #[test]
    fn every_refusal_says_which_one_it_is() {
        // A row that simply cannot be marked, with no word for why, reads as a bug.
        assert_eq!(
            sweep_refusal(&Mark::Refused(SweepRefusal::Primary)),
            Some("the repository itself")
        );
        assert_eq!(
            sweep_refusal(&Mark::Refused(SweepRefusal::Running)),
            Some("panes are running in it")
        );
        assert_eq!(
            sweep_refusal(&Mark::Refused(SweepRefusal::Removing)),
            Some("already being removed")
        );
        assert_eq!(
            sweep_refusal(&Mark::Going(Reason::Gone)),
            None,
            "nothing else has one to give"
        );
    }
    /// A snapshot with one ordinary tab, one zoomed tab and one workspace, so that
    /// [`crate::domain::dest::destinations`] offers every kind of destination there is a sentence for.
    fn offered() -> crate::port::Snapshot {
        serde_json::from_value(serde_json::json!({
            "version": "0.7.4",
            "protocol": 16,
            "workspaces": [
                {"workspace_id": "w1", "label": "app", "number": 1, "focused": true,
                 "active_tab_id": "w1:t1", "agent_status": "idle"}
            ],
            "tabs": [
                {"tab_id": "w1:t1", "workspace_id": "w1", "label": "agents", "number": 1,
                 "focused": true, "pane_count": 1, "agent_status": "idle"},
                {"tab_id": "w3:t1", "workspace_id": "w3", "label": "zoomed", "number": 1,
                 "focused": false, "pane_count": 1, "agent_status": "idle"}
            ],
            "panes": [
                {"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1",
                 "terminal_id": "t1", "focused": true, "agent_status": "idle"}
            ],
            "layouts": [
                {"tab_id": "w1:t1", "workspace_id": "w1", "zoomed": false,
                 "area": {"x": 0, "y": 0, "width": 100, "height": 40},
                 "focused_pane_id": "w1:p1",
                 "panes": [{"pane_id": "w1:p1", "focused": true,
                            "rect": {"x": 0, "y": 0, "width": 100, "height": 40}}]},
                {"tab_id": "w3:t1", "workspace_id": "w3", "zoomed": true,
                 "area": {"x": 0, "y": 0, "width": 100, "height": 40},
                 "focused_pane_id": "w3:p1",
                 "panes": [{"pane_id": "w3:p1", "focused": true,
                            "rect": {"x": 0, "y": 0, "width": 100, "height": 40}}]}
            ]
        }))
        .expect("snapshot fixture should deserialize")
    }

    /// Every sentence a destination gets, built from the destinations the picker is
    /// actually given rather than from ones written out here.
    ///
    /// A hand-written `Destination` can say something the builder would never
    /// produce, and then the row, the caption and the breadcrumb agree with each other
    /// about a screen nobody will see. Going through the builder is what stops that.
    #[test]
    fn the_destinations_the_picker_offers_read_as_they_do_on_screen() {
        let snapshot = offered();
        let said: Vec<(String, String)> =
            crate::domain::dest::destinations(&snapshot, Some("w1:p1"))
                .iter()
                .map(|offer| {
                    let caption =
                        match crate::domain::preview::predict(&snapshot, offer, "feat/login") {
                            crate::domain::preview::Preview::Layout { at, .. }
                            | crate::domain::preview::Preview::Blocked { at, .. } => landing(&at),
                            crate::domain::preview::Preview::Unavailable => String::new(),
                        };
                    (destination(offer), caption)
                })
                .collect();

        assert_eq!(
            said,
            vec![
                ("split right".to_string(), "w1  app / agents".to_string()),
                ("split down".to_string(), "w1  app / agents".to_string()),
                // The tab herdr will not take the pane into says so in the row, and the
                // caption names the tab itself rather than repeating the marker.
                (
                    "w3 / zoomed  (zoomed)".to_string(),
                    "w3 / zoomed".to_string()
                ),
                // The row offers a new tab in the space; the caption says the same thing
                // once, rather than reading "w1  app \u{2192} new tab \u{2014} a new tab".
                (
                    "w1  app \u{2192} new tab".to_string(),
                    "w1  app \u{2014} a new tab".to_string()
                ),
                (
                    "on its own".to_string(),
                    "a space of its own \u{2014} a new space".to_string()
                ),
            ]
        );
    }
}
