//! A checkout to remove, and what its removal says when it is over.
//!
//! A removal runs in a process of its own so that it outlives the picker — see
//! `docs/adr/0014-removing-outlives-the-picker.md`. That leaves two readers to serve and one
//! set of words to serve them with. The toast is the report that always happens, because it
//! is the only one left when the picker has already closed. The prompt line is the extra
//! the picker adds when it is still up to read the answer.
//!
//! [`Removal`] is the other end of the same story: the checkout, and the panes that have to
//! stop before git walks it. It lives beside the words rather than in the view that asks the
//! question, because `app` starts the removal and should not reach up into `ui` for the
//! value describing it.

use crate::domain::model::{CheckoutPath, PaneNode, RepoKey, Tree, WorktreeNode};
use crate::port::{Notification, NotificationSound, RemovalOutcome};

/// A checkout to remove: what to say about it, and what has to stop first.
///
/// One value rather than five arguments travelling together, because they have to agree.
/// `app::removals::Removals::remove` closes the panes *this* names and then removes the
/// checkout *this* names, so a pane list belonging to some other checkout removes a working
/// tree out from under everything still running in it — the accident
/// `docs/adr/0010-closing-the-panes-first.md` exists to prevent.
///
/// Every field is private and [`Removal::of`] and [`Removal::sweeping`] are the only things
/// that fill them, together, from one `WorktreeNode`. So the pairing cannot be taken apart
/// afterwards: there is no shorter list to substitute and no other checkout to repoint at.
///
/// What that does *not* reach is a `WorktreeNode` that describes a checkout falsely, since
/// its own fields are public — but such a node is a lie to the whole tree, and every row,
/// marker and count drawn from it is wrong long before this type sees it. The production
/// callers are `ui::state::PanesState::ask_to_remove` and [`SweepRemoval::of`];
/// `PanesState::replace_tree` withdraws either question if the tree changes while it is up,
/// so a `y` never acts on a list the user was not shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    repo_root: String,
    checkout_path: CheckoutPath,
    label: String,
    panes: Vec<PaneNode>,
    delete_branch: bool,
}

impl Removal {
    /// Everything removing this checkout needs, taken from the checkout itself. The branch
    /// stays: `Shift-D` keeps it — `docs/adr/0008-removing-a-worktree.md`.
    pub fn of(repo_root: &str, worktree: &WorktreeNode) -> Self {
        Self {
            repo_root: repo_root.to_string(),
            checkout_path: CheckoutPath::of(worktree),
            label: worktree.label().to_string(),
            panes: worktree.panes.clone(),
            delete_branch: false,
        }
    }

    /// The same, for a sweep, where the branch goes after the checkout
    /// (`docs/adr/0011-what-may-be-swept.md`) — when there is one. A checkout with no branch
    /// has nothing to delete, and its `label()` is a directory name, which must never reach
    /// `git branch -d`.
    pub fn sweeping(repo_root: &str, worktree: &WorktreeNode) -> Self {
        Self {
            delete_branch: worktree.branch.is_some(),
            ..Self::of(repo_root, worktree)
        }
    }

    pub fn repo_root(&self) -> &str {
        &self.repo_root
    }

    pub fn checkout_path(&self) -> &CheckoutPath {
        &self.checkout_path
    }

    /// The branch name, for the question and for saying what went.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The panes that stop. Named in the question because uncommitted work is git's to
    /// protect and this is not: whatever a working agent has in flight has no other net.
    pub fn panes(&self) -> &[PaneNode] {
        &self.panes
    }

    /// Whether `git branch -d` on `label` follows the checkout. Only [`Removal::sweeping`]
    /// sets it.
    pub fn delete_branch(&self) -> bool {
        self.delete_branch
    }
}

/// What a sweep's `y` removes: one [`Removal::sweeping`] per marked row, built from the tree
/// the box was drawn over — ADR 0011's "nothing is deleted that was not on the screen with a
/// mark against it" (`docs/adr/0011-what-may-be-swept.md`). A key the tree no longer has is
/// skipped: there is nothing at it to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepRemoval(Vec<Removal>);

impl SweepRemoval {
    pub fn of(tree: &Tree, chosen: &[(RepoKey, CheckoutPath)]) -> Self {
        let removals = chosen
            .iter()
            .filter_map(|key| {
                let (repo, worktree) = tree.find_checkout(key)?;
                Some(Removal::sweeping(&repo.repo_root, worktree))
            })
            .collect();
        Self(removals)
    }

    /// In the order the box lists them, which is repository and path order.
    pub fn removals(&self) -> &[Removal] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many of these delete a branch after the checkout: the rows that have one.
    pub fn branches(&self) -> usize {
        self.0
            .iter()
            .filter(|removal| removal.delete_branch())
            .count()
    }
}

/// What a report line starts with when the checkout went.
const REMOVED: &str = "removed";
/// What it starts with when git declined, followed by git's own words.
const REFUSED: &str = "refused ";
/// What it starts with when the checkout went and `git branch -d` then declined, followed by
/// git's own words.
const KEPT: &str = "kept ";

/// The one line a detached removal writes for whoever started it.
///
/// It is the only channel between the two processes and it has to survive the reader having
/// walked away, so it is deliberately trivial: one line, no framing, nothing to get out of
/// step. git's message is folded onto that line because a toast shows it on one anyway.
pub fn report_line(outcome: &RemovalOutcome) -> String {
    match outcome {
        RemovalOutcome::Removed => REMOVED.to_string(),
        RemovalOutcome::Refused(reason) => format!("{REFUSED}{}", folded(reason)),
        RemovalOutcome::BranchKept(reason) => format!("{KEPT}{}", folded(reason)),
    }
}

/// git's words on one line, which is all the channel carries.
fn folded(reason: &str) -> String {
    let lines: Vec<&str> = reason.lines().map(str::trim).collect();
    lines.join(" ")
}

/// Read a report line back. `None` for anything else on that channel — a line the picker
/// cannot read is not an outcome, and guessing that it meant success is how a checkout that
/// is still there stops being reported.
pub fn parse_report(line: &str) -> Option<RemovalOutcome> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line == REMOVED {
        return Some(RemovalOutcome::Removed);
    }
    if let Some(reason) = line.strip_prefix(REFUSED) {
        return Some(RemovalOutcome::Refused(reason.to_string()));
    }
    line.strip_prefix(KEPT)
        .map(|reason| RemovalOutcome::BranchKept(reason.to_string()))
}

/// The toast a finished removal shows.
///
/// It names the branch rather than the plugin, because the branch is what the reader was
/// waiting on. The path goes in the body: it is what actually went, and it is what tells two
/// checkouts of the same branch name apart.
///
/// `panes_closed` is how many panes were stopped to get this far, and it appears only when
/// the removal was then refused — see `refusal`.
pub fn notification(
    label: &str,
    checkout_path: &str,
    outcome: &RemovalOutcome,
    panes_closed: usize,
) -> Notification {
    match outcome {
        RemovalOutcome::Removed => Notification {
            title: format!("removed {label}"),
            body: Some(checkout_path.to_string()),
            // Tidying up is done often, and a chime for every checkout that goes is noise.
            sound: NotificationSound::None,
        },
        RemovalOutcome::Refused(reason) => Notification {
            title: format!("could not remove {label}"),
            // git's words, and what it cost to reach them. Not a summary of what git said:
            // the reason it gave is what says what would have been lost.
            body: Some(refusal(reason, panes_closed)),
            // The one that has to reach someone who is no longer looking.
            sound: NotificationSound::Request,
        },
        RemovalOutcome::BranchKept(reason) => Notification {
            title: format!("removed {label}, branch kept"),
            body: Some(format!("{checkout_path} — {reason}")),
            // The checkout the user asked about went, and the branch is visible in the
            // branches view.
            sound: NotificationSound::None,
        },
    }
}

/// A refusal, and what it cost to reach it.
///
/// A removal that stopped panes and then failed is the one failure that is not "nothing
/// happened": the panes are gone and the checkout is not. Saying only what git said would
/// leave the reader to reconcile it with work that has stopped for no visible reason —
/// herdr collapses the emptied tab, so there is not even an empty one left to explain it.
/// Nothing is added when the removal worked: the panes were named in the question, and the
/// checkout going is the answer.
pub fn refusal(reason: &str, panes_closed: usize) -> String {
    match panes_closed {
        0 => reason.to_string(),
        1 => format!("{reason} — its 1 pane was closed first"),
        many => format!("{reason} — its {many} panes were closed first"),
    }
}

/// What the picker says when closing the panes stopped partway.
///
/// The same rule as `refusal`, for the failure that happens one step earlier: the panes that
/// were reached are gone, the checkout is untouched, and the removal never started. herdr's
/// own words alone would leave the reader with work that has stopped and no account of why.
pub fn interrupted(pane_id: &str, reason: &str, closed: usize, total: usize) -> String {
    let so_far = match (closed, total) {
        (0, 1) => "its 1 pane was not closed".to_string(),
        (0, total) => format!("none of its {total} panes were closed"),
        (1, total) => format!("1 of its {total} panes was closed first"),
        (many, total) => format!("{many} of its {total} panes were closed first"),
    };
    format!("could not close {pane_id}: {reason} — {so_far}, and the checkout was not removed")
}

/// What the picker puts on its prompt line, when it is still up to read the answer.
///
/// Nothing on success: the row leaving the list is the report, and repeating it there would
/// only say twice what the toast has already said once. A branch that stayed is said, since
/// the row leaving says nothing about it.
pub fn message(label: &str, outcome: &RemovalOutcome, panes_closed: usize) -> Option<String> {
    match outcome {
        RemovalOutcome::Removed => None,
        // Several removals can be in flight at once, so the reason has to name its own.
        RemovalOutcome::Refused(reason) => Some(format!(
            "could not remove {label}: {}",
            refusal(reason, panes_closed)
        )),
        RemovalOutcome::BranchKept(reason) => {
            Some(format!("removed {label}, branch kept: {reason}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::WorktreeNode;
    use crate::port::{NotificationSound, RemovalOutcome};

    const REFUSAL: &str = "`git worktree remove /wt/fix-crash` failed: fatal: '/wt/fix-crash' \
                           contains modified or untracked files, use --force to delete it";

    const KEPT_WHY: &str = "error: the branch 'fix/crash' is not fully merged \
                            (`git branch -d fix/crash`)";

    /// A checkout with nothing running in it, on `branch` or on nothing.
    fn checkout(branch: Option<&str>) -> WorktreeNode {
        WorktreeNode {
            branch: branch.map(str::to_string),
            checkout_path: "/wt/fix-crash".to_string(),
            is_primary: false,
            open_workspace_id: None,
            track: None,
            panes: Vec::new(),
        }
    }

    /// One repository: `fix/crash` on a branch, and a checkout with nothing out beside it.
    fn tree() -> Tree {
        Tree {
            repos: vec![crate::domain::model::RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                refs: crate::domain::model::Refs::Read,
                worktrees: vec![
                    checkout(Some("fix/crash")),
                    WorktreeNode {
                        checkout_path: "/wt/scratch".to_string(),
                        ..checkout(None)
                    },
                ],
            }],
            ungrouped: Vec::new(),
        }
    }

    fn key(tree: &Tree, path: &str) -> (RepoKey, CheckoutPath) {
        (RepoKey::of(&tree.repos[0]), CheckoutPath::for_test(path))
    }

    #[test]
    fn a_sweep_skips_a_key_the_tree_no_longer_has() {
        // Between the box and the `y` nothing replaces the tree without withdrawing the
        // question; this is the last line of the same promise, for a key with nothing at it.
        let tree = tree();
        let sweep = SweepRemoval::of(
            &tree,
            &[key(&tree, "/wt/fix-crash"), key(&tree, "/wt/gone-already")],
        );
        let paths: Vec<&str> = sweep
            .removals()
            .iter()
            .map(|removal| removal.checkout_path().as_str())
            .collect();
        assert_eq!(paths, ["/wt/fix-crash"]);
        assert_eq!(sweep.len(), 1);
        assert!(!sweep.is_empty());
    }

    #[test]
    fn a_sweep_counts_a_branch_only_where_the_row_has_one() {
        let tree = tree();
        let sweep = SweepRemoval::of(
            &tree,
            &[key(&tree, "/wt/fix-crash"), key(&tree, "/wt/scratch")],
        );
        assert_eq!(sweep.len(), 2);
        assert_eq!(
            sweep.branches(),
            1,
            "the checkout with nothing out has none to delete"
        );
        assert!(sweep.removals()[0].delete_branch());
        assert!(!sweep.removals()[1].delete_branch());
    }

    #[test]
    fn a_removal_that_worked_survives_the_trip_between_the_processes() {
        let line = report_line(&RemovalOutcome::Removed);
        assert_eq!(parse_report(&line), Some(RemovalOutcome::Removed));
    }

    #[test]
    fn a_kept_branch_carries_gits_words_across_unchanged() {
        let line = report_line(&RemovalOutcome::BranchKept(KEPT_WHY.to_string()));
        assert_eq!(
            parse_report(&line),
            Some(RemovalOutcome::BranchKept(KEPT_WHY.to_string()))
        );
    }

    #[test]
    fn a_kept_branch_whose_reason_spans_lines_still_travels_as_one() {
        // The same fold a refusal gets: however many lines the reason had, the channel is
        // one.
        let line = report_line(&RemovalOutcome::BranchKept(
            "error: the branch 'fix/crash' is not fully merged\nhint: run 'git branch -D'"
                .to_string(),
        ));
        assert_eq!(line.lines().count(), 1, "the channel is a single line");
        assert_eq!(
            parse_report(&line),
            Some(RemovalOutcome::BranchKept(
                "error: the branch 'fix/crash' is not fully merged hint: run 'git branch -D'"
                    .to_string()
            ))
        );
    }

    #[test]
    fn a_kept_branch_is_a_removal_that_says_so_without_a_sound() {
        // The checkout the user asked about went; the branch is still in the branches view.
        let notification = notification(
            "fix/crash",
            "~/.herdr/worktrees/app/fix-crash",
            &RemovalOutcome::BranchKept(KEPT_WHY.to_string()),
            2,
        );
        assert_eq!(notification.title, "removed fix/crash, branch kept");
        assert_eq!(
            notification.body.as_deref(),
            Some(format!("~/.herdr/worktrees/app/fix-crash — {KEPT_WHY}").as_str()),
            "the path that went, then git's words on the branch that did not"
        );
        assert_eq!(notification.sound, NotificationSound::None);
        assert_eq!(
            message(
                "fix/crash",
                &RemovalOutcome::BranchKept(KEPT_WHY.to_string()),
                2
            ),
            Some(format!("removed fix/crash, branch kept: {KEPT_WHY}")),
            "and the panes are not mentioned: the checkout went, so they were not closed for nothing"
        );
    }

    #[test]
    fn only_a_sweep_of_a_checkout_on_a_branch_asks_for_the_branch_to_go() {
        // A checkout with no branch has a directory name for a label, and that name must
        // never reach `git branch -d`.
        assert!(Removal::sweeping("/src/app", &checkout(Some("fix/crash"))).delete_branch());
        assert!(!Removal::sweeping("/src/app", &checkout(None)).delete_branch());
        assert!(
            !Removal::of("/src/app", &checkout(Some("fix/crash"))).delete_branch(),
            "`Shift-D` keeps the branch"
        );
    }

    #[test]
    fn a_refusal_carries_gits_words_across_unchanged() {
        let line = report_line(&RemovalOutcome::Refused(REFUSAL.to_string()));
        assert_eq!(
            parse_report(&line),
            Some(RemovalOutcome::Refused(REFUSAL.to_string()))
        );
    }

    #[test]
    fn a_refusal_that_spans_lines_still_travels_as_one() {
        let line = report_line(&RemovalOutcome::Refused(
            "fatal: one\nfatal: two".to_string(),
        ));
        assert_eq!(line.lines().count(), 1, "the channel is a single line");
        assert_eq!(
            parse_report(&line),
            Some(RemovalOutcome::Refused("fatal: one fatal: two".to_string()))
        );
    }

    #[test]
    fn anything_else_on_that_channel_is_not_an_outcome() {
        // A line the picker cannot read is not a removal it can report on. Saying so beats
        // guessing that silence meant success.
        assert_eq!(parse_report(""), None);
        assert_eq!(parse_report("Killed"), None);
    }

    #[test]
    fn a_refusal_after_panes_were_closed_says_that_they_were() {
        // The one case where a failure is not simply "nothing happened": the panes are
        // already gone by the time git speaks, and a report that only quoted git would
        // leave the user to work that out from an empty tab.
        let notification = notification(
            "fix/crash",
            "~/.herdr/worktrees/app/fix-crash",
            &RemovalOutcome::Refused(REFUSAL.to_string()),
            2,
        );
        assert_eq!(
            notification.body.as_deref(),
            Some(format!("{REFUSAL} — its 2 panes were closed first").as_str())
        );
        assert_eq!(
            message(
                "fix/crash",
                &RemovalOutcome::Refused(REFUSAL.to_string()),
                1
            ),
            Some(format!(
                "could not remove fix/crash: {REFUSAL} — its 1 pane was closed first"
            ))
        );
    }

    #[test]
    fn a_close_that_stopped_partway_says_how_far_it_got() {
        // The other half of the same rule: panes are gone, the checkout is not, and the
        // reader would otherwise have herdr's bare refusal and an emptied tab to reconcile.
        assert_eq!(
            interrupted(
                "w1:p3",
                "herdr rejected pane.close: no such pane (not_found)",
                1,
                3
            ),
            "could not close w1:p3: herdr rejected pane.close: no such pane (not_found) \
             — 1 of its 3 panes was closed first, and the checkout was not removed"
        );
        assert_eq!(
            interrupted("w1:p1", "gone", 0, 2),
            "could not close w1:p1: gone — none of its 2 panes were closed, and the \
             checkout was not removed"
        );
        assert_eq!(
            interrupted("w1:p1", "gone", 0, 1),
            "could not close w1:p1: gone — its 1 pane was not closed, and the checkout \
             was not removed"
        );
        assert_eq!(
            interrupted("w1:p3", "gone", 2, 3),
            "could not close w1:p3: gone — 2 of its 3 panes were closed first, and the \
             checkout was not removed"
        );
    }

    #[test]
    fn a_removal_that_worked_does_not_dwell_on_the_panes() {
        // They were listed in the question and the checkout is gone; saying it again is
        // saying twice what the row leaving the list already said once.
        let notification = notification(
            "fix/crash",
            "~/.herdr/worktrees/app/fix-crash",
            &RemovalOutcome::Removed,
            2,
        );
        assert_eq!(
            notification.body.as_deref(),
            Some("~/.herdr/worktrees/app/fix-crash")
        );
        assert_eq!(message("fix/crash", &RemovalOutcome::Removed, 2), None);
    }

    #[test]
    fn the_toast_names_the_branch_and_points_at_the_path() {
        let notification = notification(
            "fix/crash",
            "~/.herdr/worktrees/app/fix-crash",
            &RemovalOutcome::Removed,
            0,
        );
        assert_eq!(notification.title, "removed fix/crash");
        assert_eq!(
            notification.body.as_deref(),
            Some("~/.herdr/worktrees/app/fix-crash")
        );
        assert_eq!(
            notification.sound,
            NotificationSound::None,
            "tidying up is done often; a chime every time would be noise"
        );
    }

    #[test]
    fn the_refused_one_is_the_one_that_makes_a_sound() {
        let notification = notification(
            "fix/crash",
            "~/.herdr/worktrees/app/fix-crash",
            &RemovalOutcome::Refused(REFUSAL.to_string()),
            0,
        );
        assert_eq!(notification.title, "could not remove fix/crash");
        assert_eq!(
            notification.body.as_deref(),
            Some(REFUSAL),
            "git's words, not a summary of them"
        );
        assert_eq!(notification.sound, NotificationSound::Request);
    }

    #[test]
    fn the_picker_says_nothing_when_the_row_simply_leaves() {
        assert_eq!(message("fix/crash", &RemovalOutcome::Removed, 0), None);
    }

    #[test]
    fn the_picker_repeats_the_refusal_and_says_which_checkout_it_was_about() {
        // Several removals can be in flight at once, so the reason has to name its own.
        assert_eq!(
            message(
                "fix/crash",
                &RemovalOutcome::Refused(REFUSAL.to_string()),
                0
            ),
            Some(format!("could not remove fix/crash: {REFUSAL}"))
        );
    }

    #[test]
    fn a_sweep_removes_under_the_repository_its_key_names_when_two_list_one_path() {
        // Issue #46's pair: a second repository listing the same path, and a key naming it.
        let mut tree = tree();
        tree.repos.push(crate::domain::model::RepoNode {
            repo_key: "/src/old/.git".into(),
            repo_root: "/src/old".into(),
            display_name: "me/old".into(),
            refs: crate::domain::model::Refs::Read,
            worktrees: vec![checkout(Some("chore/deps"))],
        });
        let sweep = SweepRemoval::of(
            &tree,
            &[(
                RepoKey::of(&tree.repos[1]),
                CheckoutPath::for_test("/wt/fix-crash"),
            )],
        );
        assert_eq!(sweep.len(), 1);
        assert_eq!(sweep.removals()[0].repo_root(), "/src/old");
        assert_eq!(sweep.removals()[0].label(), "chore/deps");
    }
}
