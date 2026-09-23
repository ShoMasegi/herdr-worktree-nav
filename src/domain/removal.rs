//! A checkout to remove, and the one line its removal reports itself on.
//!
//! A removal runs in a process of its own so that it outlives the picker — see
//! `docs/adr/0014-removing-outlives-the-picker.md`. [`report_line`] and [`parse_report`] are
//! the channel between the two processes: a wire format rather than a sentence, which is why
//! they are here and the words a reader sees are in
//! [`app::words`](crate::app::words).
//!
//! [`Removal`] is the other end of the same story: the checkout, and the panes that have to
//! stop before git walks it. It lives here rather than in the view that asks the question,
//! because `app` starts the removal and should not reach up into `ui` for the value
//! describing it.

use crate::domain::model::{CheckoutPath, PaneNode, RepoKey, Tree, WorktreeNode};
use crate::port::RemovalOutcome;

/// A checkout to remove: what to say about it, and what has to stop first.
///
/// One value rather than five arguments travelling together, because they have to agree.
/// [`app::removals::Removals::remove`](crate::app::removals::Removals::remove) closes the
/// panes *this* names and then removes the checkout *this* names, so a pane list belonging to
/// some other checkout removes a working tree out from under everything still running in it —
/// the accident `docs/adr/0010-closing-the-panes-first.md` exists to prevent.
///
/// Every field is private and [`Removal::of`] and [`Removal::sweeping`] are the only things
/// that fill them, together, from one `WorktreeNode`. So the pairing cannot be taken apart
/// afterwards: there is no shorter list to substitute and no other checkout to repoint at.
///
/// What that does *not* reach is a `WorktreeNode` that describes a checkout falsely, since its
/// own fields are public — but such a node is a lie to the whole tree, and every row, marker
/// and count drawn from it is wrong long before this type sees it. The production callers are
/// [`ui::panes::PanesState::ask_to_remove`](crate::ui::panes::PanesState::ask_to_remove) and
/// [`SweepRemoval::of`];
/// [`PanesState::replace_tree`](crate::ui::panes::PanesState::replace_tree) withdraws either
/// question if the tree changes while it is up, so a `y` never acts on a list the user was not
/// shown.
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
            delete_branch: worktree.branch.name().is_some(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{Branch, Position, WorktreeNode};
    use crate::port::RemovalOutcome;

    const REFUSAL: &str = "`git worktree remove /wt/fix-crash` failed: fatal: '/wt/fix-crash' \
                           contains modified or untracked files, use --force to delete it";

    const KEPT_WHY: &str = "error: the branch 'fix/crash' is not fully merged \
                            (`git branch -d fix/crash`)";

    /// A checkout with nothing running in it, on `branch` or on nothing.
    fn checkout(branch: Option<&str>) -> WorktreeNode {
        WorktreeNode {
            branch: branch.map_or(Branch::NothingOut, |b| Branch::Out(b.to_string())),
            checkout_path: "/wt/fix-crash".to_string(),
            is_primary: false,
            open_workspace_id: None,
            position: Position::NotSaid,
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
    fn only_a_sweep_of_a_checkout_on_a_branch_asks_for_the_branch_to_go() {
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
        assert_eq!(parse_report(""), None);
        assert_eq!(parse_report("Killed"), None);
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
