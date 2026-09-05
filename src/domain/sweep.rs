//! Which checkouts a sweep may offer to delete, and why.
//!
//! `Shift-D` never had to answer this: it acts on the row under the cursor because a person
//! put the cursor there. A sweep has to decide for itself, so the reasoning is here, pure,
//! where it can be read and tested without a git or a `gh` —
//! `docs/adr/0011-what-may-be-swept.md` is the decision this carries out.
//!
//! Two rules shape everything below. **A mark is a suggestion with its reason attached**, so
//! nothing is offered without a `Reason` to show beside it. And **`gh` may only widen**: it
//! never clears a mark git put there, never gates the mode, and when it cannot be asked the
//! rows it would have judged say so rather than looking like rows with nothing to find.

use std::collections::BTreeMap;

use crate::domain::model::{Tree, WorkingTree};
use crate::port::{PullRequestOutcome, SettledPullRequest, Track};

/// Why a checkout is offered for deletion. Shown beside the mark, because a mark whose
/// reason is invisible is one the user either trusts blindly or clears wholesale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// git cannot find the ref this branch tracks — the ordinary end of a branch whose pull
    /// request was merged and whose head the remote then deleted.
    Gone,
    /// The branch's pull request is finished with. Only reached where git had nothing to
    /// say, since `gh` widens and never overrides.
    PullRequest {
        number: u64,
        outcome: PullRequestOutcome,
    },
}

impl Reason {
    /// What the row says beside its mark.
    pub fn label(&self) -> String {
        match self {
            Reason::Gone => "gone".to_string(),
            Reason::PullRequest { number, outcome } => {
                let what = match outcome {
                    PullRequestOutcome::Merged => "merged",
                    PullRequestOutcome::Closed => "closed",
                };
                format!("PR #{number} {what}")
            }
        }
    }
}

/// Why a checkout can never be swept, whatever the user presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The repository's own checkout. `git worktree remove` will not take it, and the branch
    /// it is on is the one everything else is measured against.
    Primary,
    /// Panes are running in it. Closing somebody's panes is one deliberate act — see
    /// `docs/adr/0010-closing-the-panes-first.md` — and a batch is not where it belongs.
    Running,
    /// Its removal is already going, in a process of its own.
    Removing,
}

impl Refusal {
    /// What the row says instead of a mark.
    pub fn label(self) -> &'static str {
        match self {
            Refusal::Primary => "the repository itself",
            Refusal::Running => "panes are running in it",
            Refusal::Removing => "already being removed",
        }
    }
}

/// What a sweep may do with one checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidate {
    /// Marked when the sweep opens.
    Offered(Reason),
    /// Nothing found that says it should go — but `gh` could not be asked, and this is a row
    /// its answer might have changed. The row says so, because a sweep that quietly finds
    /// less when a dependency is missing is worse than one that says which half it is
    /// missing.
    Unjudged,
    /// Nothing says it should go. `Space` still marks it: disagreeing with the sweep is the
    /// same act as widening it, one row at a time.
    Available,
    /// Never swept.
    Refused(Refusal),
}

impl Candidate {
    /// Whether the sweep marks this when it opens.
    pub fn is_offered(&self) -> bool {
        matches!(self, Candidate::Offered(_))
    }

    /// Whether the user may mark it at all.
    pub fn is_markable(&self) -> bool {
        !matches!(self, Candidate::Refused(_))
    }
}

/// Everything a sweep decides on that is not in the tree.
pub struct Facts<'a> {
    /// What git said about each working tree, by checkout path. Absent is "not asked yet",
    /// which is not clean — see `domain::model::WorkingTree`.
    pub working_trees: &'a BTreeMap<String, WorkingTree>,
    /// What `gh` said, by repository root. `None` for a repository `gh` could not be asked
    /// about; a repository absent from the map has not been asked.
    pub settled: &'a BTreeMap<String, Option<Vec<SettledPullRequest>>>,
    /// Checkout paths whose removal is already running.
    pub removing: &'a [String],
}

/// What the sweep may do with every checkout in the tree, by checkout path.
pub fn candidates(tree: &Tree, facts: &Facts) -> BTreeMap<String, Candidate> {
    let mut out = BTreeMap::new();
    for repo in &tree.repos {
        let settled = facts.settled.get(&repo.repo_root);
        for worktree in &repo.worktrees {
            let path = &worktree.checkout_path;
            out.insert(path.clone(), judge(worktree, settled, facts));
        }
    }
    out
}

fn judge(
    worktree: &crate::domain::model::WorktreeNode,
    settled: Option<&Option<Vec<SettledPullRequest>>>,
    facts: &Facts,
) -> Candidate {
    // The refusals come first and in this order because they are about the checkout rather
    // than about whether anyone is finished with it: a running pane is a reason not to sweep
    // a branch whose upstream went, not a tie to be broken afterwards.
    if worktree.is_primary {
        return Candidate::Refused(Refusal::Primary);
    }
    if facts.removing.contains(&worktree.checkout_path) {
        return Candidate::Refused(Refusal::Removing);
    }
    if !worktree.panes.is_empty() {
        return Candidate::Refused(Refusal::Running);
    }

    // Everything below is a reason to *offer*, and every one of them needs a working tree
    // with nothing in it to lose. Clean is a positive answer: dirty, unreadable and
    // not-asked-yet are all "no", and the user may still mark those by hand for git to
    // refuse. Marking one by default would be deleting on the strength of a silence.
    let clean = facts
        .working_trees
        .get(&worktree.checkout_path)
        .is_some_and(|answer| answer.is_clean());

    if clean && worktree.track == Some(Track::Gone) {
        return Candidate::Offered(Reason::Gone);
    }

    match settled {
        // Asked, and it answered. A pull request for this branch that is finished with is
        // the second way in; anything else is simply not a candidate.
        Some(Some(pull_requests)) => {
            let found = worktree
                .branch
                .as_ref()
                .and_then(|branch| pull_requests.iter().find(|pr| &pr.head_ref == branch));
            match found {
                Some(pull_request) if clean => Candidate::Offered(Reason::PullRequest {
                    number: pull_request.number,
                    outcome: pull_request.outcome,
                }),
                _ => Candidate::Available,
            }
        }
        // Asked, and it could not answer. Only worth saying on a row the answer could have
        // changed: one already refused, or already offered by git, or that git would refuse
        // anyway, is not a row a pull request was going to decide.
        Some(None) if clean && worktree.branch.is_some() => Candidate::Unjudged,
        _ => Candidate::Available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{PaneNode, RepoNode, WorktreeNode};
    use crate::port::AgentStatus;

    fn worktree(branch: &str, path: &str) -> WorktreeNode {
        WorktreeNode {
            branch: Some(branch.to_string()),
            checkout_path: path.to_string(),
            is_primary: false,
            open_workspace_id: None,
            track: None,
            panes: Vec::new(),
        }
    }

    fn tree_of(worktrees: Vec<WorktreeNode>) -> Tree {
        Tree {
            repos: vec![RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                worktrees,
            }],
            ungrouped: Vec::new(),
        }
    }

    fn clean(paths: &[&str]) -> BTreeMap<String, WorkingTree> {
        paths
            .iter()
            .map(|path| ((*path).to_string(), WorkingTree::Clean))
            .collect()
    }

    fn asked(
        pull_requests: Vec<SettledPullRequest>,
    ) -> BTreeMap<String, Option<Vec<SettledPullRequest>>> {
        BTreeMap::from([("/src/app".to_string(), Some(pull_requests))])
    }

    fn merged(number: u64, head_ref: &str) -> SettledPullRequest {
        SettledPullRequest {
            number,
            head_ref: head_ref.to_string(),
            outcome: PullRequestOutcome::Merged,
        }
    }

    fn judged(tree: &Tree, facts: &Facts) -> BTreeMap<String, Candidate> {
        candidates(tree, facts)
    }

    /// The everything-is-fine case: one clean checkout, nothing running, nobody asked `gh`.
    fn facts<'a>(
        working_trees: &'a BTreeMap<String, WorkingTree>,
        settled: &'a BTreeMap<String, Option<Vec<SettledPullRequest>>>,
    ) -> Facts<'a> {
        Facts {
            working_trees,
            settled,
            removing: &[],
        }
    }

    #[test]
    fn a_clean_checkout_whose_upstream_is_gone_is_offered_with_its_reason() {
        let mut wt = worktree("fix/crash", "/wt/fix-crash");
        wt.track = Some(Track::Gone);
        let trees = clean(&["/wt/fix-crash"]);
        let none = BTreeMap::new();
        let judged = judged(&tree_of(vec![wt]), &facts(&trees, &none));
        assert_eq!(judged["/wt/fix-crash"], Candidate::Offered(Reason::Gone));
        assert_eq!(judged["/wt/fix-crash"].label_for_test(), "gone");
    }

    #[test]
    fn nothing_is_offered_on_a_working_tree_nobody_has_answered_for() {
        // The state the picker opens in. Offering here would be deleting a checkout because
        // a walk has not finished yet, which is the one direction that cannot be undone by
        // waiting a moment longer.
        let mut wt = worktree("fix/crash", "/wt/fix-crash");
        wt.track = Some(Track::Gone);
        let nothing = BTreeMap::new();
        let none = BTreeMap::new();
        let judged = judged(&tree_of(vec![wt]), &facts(&nothing, &none));
        assert_eq!(judged["/wt/fix-crash"], Candidate::Available);
    }

    #[test]
    fn a_working_tree_holding_work_is_not_offered_but_can_still_be_marked() {
        // git refuses it, and git's refusal is the answer rather than an obstacle: it says
        // what would have been lost. What the sweep will not do is suggest it.
        let mut wt = worktree("fix/crash", "/wt/fix-crash");
        wt.track = Some(Track::Gone);
        let dirty = BTreeMap::from([("/wt/fix-crash".to_string(), WorkingTree::Dirty)]);
        let none = BTreeMap::new();
        let judged = judged(&tree_of(vec![wt]), &facts(&dirty, &none));
        assert_eq!(judged["/wt/fix-crash"], Candidate::Available);
        assert!(judged["/wt/fix-crash"].is_markable());
    }

    #[test]
    fn the_three_checkouts_a_sweep_never_touches() {
        let mut primary = worktree("main", "/src/app");
        primary.is_primary = true;
        primary.track = Some(Track::Gone);
        let mut running = worktree("feat/login", "/wt/feat-login");
        running.track = Some(Track::Gone);
        running.panes = vec![PaneNode {
            pane_id: "w2:p1".into(),
            workspace_id: "w2".into(),
            tab_id: "w2:t1".into(),
            display_name: None,
            agent_status: AgentStatus::Idle,
            focused: false,
        }];
        let mut going = worktree("fix/crash", "/wt/fix-crash");
        going.track = Some(Track::Gone);

        let trees = clean(&["/src/app", "/wt/feat-login", "/wt/fix-crash"]);
        let none = BTreeMap::new();
        let judged = candidates(
            &tree_of(vec![primary, running, going]),
            &Facts {
                working_trees: &trees,
                settled: &none,
                removing: &["/wt/fix-crash".to_string()],
            },
        );

        // Every one of them is clean with a gone upstream, so only the refusal keeps them out.
        assert_eq!(judged["/src/app"], Candidate::Refused(Refusal::Primary));
        assert_eq!(
            judged["/wt/feat-login"],
            Candidate::Refused(Refusal::Running)
        );
        assert_eq!(
            judged["/wt/fix-crash"],
            Candidate::Refused(Refusal::Removing)
        );
        assert!(judged.values().all(|c| !c.is_markable()));
    }

    #[test]
    fn a_settled_pull_request_offers_a_branch_git_had_nothing_to_say_about() {
        // The squash-merge case, and the reason `gh` is consulted at all: the head branch
        // was kept, so `%(upstream:track)` says nothing and only GitHub knows it is over.
        let wt = worktree("feat/login", "/wt/feat-login");
        let trees = clean(&["/wt/feat-login"]);
        let settled = asked(vec![merged(123, "feat/login")]);
        let judged = judged(&tree_of(vec![wt]), &facts(&trees, &settled));
        assert_eq!(
            judged["/wt/feat-login"],
            Candidate::Offered(Reason::PullRequest {
                number: 123,
                outcome: PullRequestOutcome::Merged,
            })
        );
        assert_eq!(judged["/wt/feat-login"].label_for_test(), "PR #123 merged");
    }

    #[test]
    fn gh_widens_and_never_overrides() {
        // A branch git already called `gone` keeps git's reason even where a pull request
        // would have given another, and an open pull request — which is not in the settled
        // list at all — does not take the mark away.
        let mut wt = worktree("fix/crash", "/wt/fix-crash");
        wt.track = Some(Track::Gone);
        let trees = clean(&["/wt/fix-crash"]);
        let settled = asked(vec![merged(7, "fix/crash")]);
        let judged = judged(&tree_of(vec![wt]), &facts(&trees, &settled));
        assert_eq!(
            judged["/wt/fix-crash"],
            Candidate::Offered(Reason::Gone),
            "git said it first"
        );
    }

    #[test]
    fn a_repository_gh_could_not_be_asked_about_says_so_on_the_rows_it_would_have_judged() {
        // The visible half of ADR 0011's price. Without this the same repository simply
        // sweeps fewer rows and nothing says why.
        let judgeable = worktree("feat/login", "/wt/feat-login");
        let mut already = worktree("fix/crash", "/wt/fix-crash");
        already.track = Some(Track::Gone);
        let mut running = worktree("chore/tidy", "/wt/tidy");
        running.panes = vec![PaneNode {
            pane_id: "w3:p1".into(),
            workspace_id: "w3".into(),
            tab_id: "w3:t1".into(),
            display_name: None,
            agent_status: AgentStatus::Idle,
            focused: false,
        }];
        let trees = clean(&["/wt/feat-login", "/wt/fix-crash", "/wt/tidy"]);
        let unavailable = BTreeMap::from([("/src/app".to_string(), None)]);

        let judged = judged(
            &tree_of(vec![judgeable, already, running]),
            &facts(&trees, &unavailable),
        );
        assert_eq!(judged["/wt/feat-login"], Candidate::Unjudged);
        assert_eq!(
            judged["/wt/fix-crash"],
            Candidate::Offered(Reason::Gone),
            "git answered this one, so there was nothing to be unsure about"
        );
        assert_eq!(
            judged["/wt/tidy"],
            Candidate::Refused(Refusal::Running),
            "a pull request was never going to decide this one"
        );
    }

    #[test]
    fn asked_and_told_nothing_is_not_the_same_as_not_being_able_to_ask() {
        // `Some(vec![])` against `None`. The first is a repository with no finished pull
        // request, which is an answer; the second is no answer at all.
        let trees = clean(&["/wt/feat-login"]);
        let answered = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &asked(Vec::new())),
        );
        assert_eq!(answered["/wt/feat-login"], Candidate::Available);

        let unavailable = BTreeMap::from([("/src/app".to_string(), None)]);
        let could_not = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &unavailable),
        );
        assert_eq!(could_not["/wt/feat-login"], Candidate::Unjudged);
    }

    #[test]
    fn a_detached_checkout_is_never_offered_by_a_pull_request() {
        // Nothing points at it, so there is no head ref to match and no branch a pull
        // request could have been for.
        let mut wt = worktree("feat/login", "/wt/detached");
        wt.branch = None;
        let trees = clean(&["/wt/detached"]);
        let settled = asked(vec![merged(9, "feat/login")]);
        let judged = judged(&tree_of(vec![wt]), &facts(&trees, &settled));
        assert_eq!(judged["/wt/detached"], Candidate::Available);
    }

    impl Candidate {
        /// The reason's own words, for the tests that care what a row would say.
        fn label_for_test(&self) -> String {
            match self {
                Candidate::Offered(reason) => reason.label(),
                other => panic!("{other:?} has no reason to show"),
            }
        }
    }
}
