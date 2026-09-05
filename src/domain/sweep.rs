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

use crate::domain::model::{RepoNode, Tree, WorkingTree};
use crate::port::{PullRequestOutcome, SettledPullRequest, SettledPullRequests, Track};

/// Why a checkout is offered for deletion. Shown beside the mark, because a mark whose
/// reason is invisible is one the user either trusts blindly or clears wholesale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// git cannot find the ref this branch tracks — the ordinary end of a branch whose pull
    /// request was merged and whose head the remote then deleted.
    Gone,
    /// The branch's pull request is finished with. Only reached where git did not already
    /// offer the row itself, since `gh` widens and never overrides — which is narrower than
    /// "git had nothing to say": `Ahead`, `Behind` and `Diverged` all arrive here, so a
    /// branch carrying commits its upstream has not got can be offered on a merged pull
    /// request. ADR 0011 removes the checkout and then runs `git branch -d`, which refuses
    /// an unmerged branch, so the commits outlive the sweep — but they are not a reason the
    /// sweep weighs, and this is where they would be weighed.
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
    /// The repository's own checkout. `git worktree remove` will not take it — `fatal: is a
    /// main working tree`, whatever the branch or the working tree says.
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
    /// Nothing found that says it should go, and `gh`'s answer could not settle it.
    ///
    /// Three ways `gh` fails to settle it, and only the first is a missing dependency: it
    /// could not be asked at all; it answered but the window it was given was full, so a
    /// branch absent from the list may simply be further back than it reached; or it
    /// answered and one of the entries was in a state this does not know. The row says so in
    /// all three, because a sweep that quietly finds less when it could not look is worse
    /// than one that says which half it could not see.
    ///
    /// That enumeration is about `gh`, and is not the whole of "could not look". git has a
    /// way to fail too — a ref walk that failed leaves every checkout with `track: None`,
    /// which is also what a branch level with its upstream has — and no variant here can say
    /// so, because `Track` has nowhere to put it. See issue #21. Fixing it is a change to
    /// what a `RepoInput` can carry, and it has to land before a sweep deletes anything.
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

/// Which repository a `gh` answer is about.
///
/// A newtype rather than a `String` because `RepoNode` carries two of those side by side —
/// `repo_key` (`/src/app/.git`) and `repo_root` (`/src/app`) — and a map keyed by the wrong
/// one silently answers nothing for every checkout in the tree: no marks, no `Unjudged`, no
/// error, nothing on screen. Outside this module [`RepoRoot::of`] is the only way to make
/// one, so there is nothing for a caller to pick wrongly; inside it the field is in scope,
/// and what pins the choice is a test rather than the compiler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RepoRoot(String);

impl RepoRoot {
    pub fn of(repo: &RepoNode) -> Self {
        RepoRoot(repo.repo_root.clone())
    }
}

/// Everything a sweep decides on that is not in the tree.
pub struct Facts<'a> {
    /// What git said about each working tree, by checkout path. Absent is "not asked yet",
    /// which is not clean — see `domain::model::WorkingTree`.
    pub working_trees: &'a BTreeMap<String, WorkingTree>,
    /// What `gh` said, by repository root. `None` for a repository `gh` could not be asked
    /// about — the reason belongs on the prompt line, not in a decision — and a repository
    /// absent from the map has not been asked at all.
    pub settled: &'a BTreeMap<RepoRoot, Option<SettledPullRequests>>,
    /// Checkout paths whose removal is already running.
    pub removing: &'a [String],
}

/// What the sweep may do with every checkout in the tree, by checkout path.
pub fn candidates(tree: &Tree, facts: &Facts) -> BTreeMap<String, Candidate> {
    let mut out = BTreeMap::new();
    for repo in &tree.repos {
        let settled = facts.settled.get(&RepoRoot::of(repo));
        for worktree in &repo.worktrees {
            let path = &worktree.checkout_path;
            out.insert(path.clone(), judge(worktree, settled, facts));
        }
    }
    out
}

fn judge(
    worktree: &crate::domain::model::WorktreeNode,
    settled: Option<&Option<SettledPullRequests>>,
    facts: &Facts,
) -> Candidate {
    // The refusals come first because they are about the checkout rather than about whether
    // anyone is finished with it: a running pane is a reason not to sweep a branch whose
    // upstream went, not a tie to be broken afterwards.
    //
    // Their order among themselves decides only which sentence a checkout that earns two of
    // them shows, and it runs most permanent first: being the repository itself is not a
    // state that passes, a removal already going will end, and panes close whenever somebody
    // closes them. `which_refusal_a_checkout_that_earns_two_of_them_shows` is what says so.
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

    // Below here `gh` is the only thing left that could say anything, so a row it could not
    // reach is `Unjudged` rather than `Available` — but only where its answer would have
    // changed the outcome. A row already refused, already offered by git, or that git would
    // refuse anyway was never a row a pull request was going to decide.
    let could_have_decided = clean && worktree.branch.is_some();
    let settled = match settled {
        // Nobody has asked yet. Not the same as asking and getting nothing, but there is
        // nothing to say about it either: an answer is still on its way, and a permanent word
        // on a temporary state is what the working-tree walk already avoids.
        None => return Candidate::Available,
        // Asked, and `gh` could not answer. This is the row that says so.
        Some(None) if could_have_decided => return Candidate::Unjudged,
        Some(None) => return Candidate::Available,
        Some(Some(settled)) => settled,
    };

    // A hit and a miss are answered in the same breath, once per variant, because what a
    // *miss* means is a fact about the list rather than about the branch. Written any other
    // way the two meanings can end up in one arm, which is what a flag beside the list
    // allowed and what this shape does not.
    let found = worktree
        .branch
        .as_ref()
        .and_then(|branch| finished_with(settled.pull_requests(), branch));
    match (found, settled) {
        (Some(pull_request), _) if clean => Candidate::Offered(Reason::PullRequest {
            number: pull_request.number,
            outcome: pull_request.outcome,
        }),
        // Found, and the working tree has something in it. git would refuse the removal, so
        // the sweep does not suggest it — the same rule the `gone` path is under.
        (Some(_), _) => Candidate::Available,
        // Missing from all of them: this branch has no finished pull request.
        (None, SettledPullRequests::All(_)) => Candidate::Available,
        // Missing from as many as `gh` was asked for: the window may not reach back far
        // enough, and saying "nothing to sweep" on the strength of a page size is the
        // confident wrong claim this whole distinction exists to prevent.
        (None, SettledPullRequests::Window(_)) if could_have_decided => Candidate::Unjudged,
        (None, SettledPullRequests::Window(_)) => Candidate::Available,
    }
}

/// This repository's own finished pull request for `branch`, if it has one here.
///
/// A branch on somebody else's fork never matches. `head_ref` is a name on whichever
/// repository the pull request came from, so a contributor's merged `patch-1` says nothing
/// about a local checkout of the same name, and matching it would delete work on the strength
/// of a coincidence. `gh` may only widen a sweep, and a wrong mark is not a widening.
///
/// A branch can have more than one — closed, then reopened and merged, or a name used twice —
/// and taking whichever `gh` happened to list first would make the reason on the row depend
/// on a sort order nothing here pins. Merged wins, because a branch with a merge behind it
/// has landed whatever else also happened to it; between two of a kind the later number is
/// the later story.
fn finished_with<'a>(
    pull_requests: &'a [SettledPullRequest],
    branch: &str,
) -> Option<&'a SettledPullRequest> {
    pull_requests
        .iter()
        .filter(|pull_request| !pull_request.from_a_fork && pull_request.head_ref == branch)
        .max_by_key(|pull_request| {
            (
                pull_request.outcome == PullRequestOutcome::Merged,
                pull_request.number,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{PaneNode, RepoNode, WorktreeNode};
    use crate::port::{AgentStatus, SettledPullRequest};

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

    /// The one repository these mostly work with, so the key and the tree cannot drift.
    fn only_repo() -> RepoNode {
        RepoNode {
            repo_key: "/src/app/.git".into(),
            repo_root: "/src/app".into(),
            display_name: "me/app".into(),
            worktrees: Vec::new(),
        }
    }

    fn tree_of(worktrees: Vec<WorktreeNode>) -> Tree {
        Tree {
            repos: vec![RepoNode {
                worktrees,
                ..only_repo()
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
    ) -> BTreeMap<RepoRoot, Option<SettledPullRequests>> {
        told(pull_requests, true)
    }

    /// What `gh` said, and whether it was all of it.
    fn told(
        pull_requests: Vec<SettledPullRequest>,
        complete: bool,
    ) -> BTreeMap<RepoRoot, Option<SettledPullRequests>> {
        BTreeMap::from([(
            RepoRoot::of(&only_repo()),
            Some(if complete {
                SettledPullRequests::All(pull_requests)
            } else {
                SettledPullRequests::Window(pull_requests)
            }),
        )])
    }

    fn merged(number: u64, head_ref: &str) -> SettledPullRequest {
        settled(number, head_ref, PullRequestOutcome::Merged)
    }

    fn settled(number: u64, head_ref: &str, outcome: PullRequestOutcome) -> SettledPullRequest {
        SettledPullRequest {
            number,
            head_ref: head_ref.to_string(),
            from_a_fork: false,
            outcome,
        }
    }

    fn judged(tree: &Tree, facts: &Facts) -> BTreeMap<String, Candidate> {
        candidates(tree, facts)
    }

    /// The everything-is-fine case: one clean checkout, nothing running, nobody asked `gh`.
    fn facts<'a>(
        working_trees: &'a BTreeMap<String, WorkingTree>,
        settled: &'a BTreeMap<RepoRoot, Option<SettledPullRequests>>,
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
    fn a_working_tree_git_would_not_read_is_never_offered() {
        // Not the same as reading it and finding nothing. `safe.directory`, or a checkout
        // whose directory has gone: git said it could not look, and offering on that is
        // offering to delete whatever was in there on the strength of a failed question.
        let mut wt = worktree("fix/crash", "/wt/fix-crash");
        wt.track = Some(Track::Gone);
        let unreadable = BTreeMap::from([("/wt/fix-crash".to_string(), WorkingTree::Unreadable)]);
        let none = BTreeMap::new();
        let judged = judged(&tree_of(vec![wt]), &facts(&unreadable, &none));
        assert_eq!(judged["/wt/fix-crash"], Candidate::Available);
    }

    #[test]
    fn a_detached_checkout_is_not_called_unjudged_when_gh_could_not_be_asked() {
        // Nothing points at it, so there was never a pull request that could have decided
        // it. Saying "PR unknown" would blame `gh` for a silence git is responsible for.
        let mut wt = worktree("feat/login", "/wt/detached");
        wt.branch = None;
        let trees = clean(&["/wt/detached"]);
        let unavailable = BTreeMap::from([(RepoRoot::of(&only_repo()), None)]);
        let judged = judged(&tree_of(vec![wt]), &facts(&trees, &unavailable));
        assert_eq!(judged["/wt/detached"], Candidate::Available);
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
        let unavailable = BTreeMap::from([(RepoRoot::of(&only_repo()), None)]);

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

        let unavailable = BTreeMap::from([(RepoRoot::of(&only_repo()), None)]);
        let could_not = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &unavailable),
        );
        assert_eq!(could_not["/wt/feat-login"], Candidate::Unjudged);
    }

    #[test]
    fn a_branch_beyond_the_window_gh_was_given_is_not_called_finished_with() {
        // `gh` answers newest first and says nothing when it truncates, so "not in this
        // list" from a full window is not "no pull request". Reading it as one would tell
        // the user there is nothing to sweep on the strength of a page size.
        let trees = clean(&["/wt/feat-login"]);
        let truncated = told(vec![merged(1, "some/other")], false);
        let partial = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &truncated),
        );
        assert_eq!(partial["/wt/feat-login"], Candidate::Unjudged);

        // And the same list, known to be all of them, is an answer.
        let whole = told(vec![merged(1, "some/other")], true);
        let complete = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &whole),
        );
        assert_eq!(complete["/wt/feat-login"], Candidate::Available);
    }

    #[test]
    fn a_branch_found_in_a_truncated_window_is_still_an_answer() {
        // Truncation only casts doubt on absence. A pull request that *is* there was seen.
        let trees = clean(&["/wt/feat-login"]);
        let truncated = told(vec![merged(4, "feat/login")], false);
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &truncated),
        );
        assert_eq!(
            judged["/wt/feat-login"],
            Candidate::Offered(Reason::PullRequest {
                number: 4,
                outcome: PullRequestOutcome::Merged,
            })
        );
    }

    #[test]
    fn a_repository_nobody_has_asked_gh_about_yet_is_not_reported_as_unseen() {
        // An answer is still on its way. Saying "PR unknown" here would put a permanent
        // word on a temporary state, which is the mistake the working-tree walk already
        // avoids by drawing no marker until it knows.
        let trees = clean(&["/wt/feat-login"]);
        let none = BTreeMap::new();
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &none),
        );
        assert_eq!(judged["/wt/feat-login"], Candidate::Available);
    }

    #[test]
    fn a_pull_request_that_was_closed_is_not_reported_as_one_that_landed() {
        // The two outcomes read very differently to whoever is deciding. "PR #4 merged"
        // says the work is in; "PR #4 closed" says it was abandoned. Getting it the wrong
        // way round tells someone their work landed as they delete the only copy of it.
        let trees = clean(&["/wt/feat-login"]);
        let abandoned = asked(vec![settled(4, "feat/login", PullRequestOutcome::Closed)]);
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &abandoned),
        );
        assert_eq!(judged["/wt/feat-login"].label_for_test(), "PR #4 closed");
    }

    #[test]
    fn one_repositorys_pull_requests_never_judge_anothers_checkouts() {
        // `feat/login` in two repositories is ordinary, and only one of them was asked
        // about. Answering for both would offer to delete a checkout on the strength of a
        // merge that happened somewhere else entirely.
        let tree = Tree {
            repos: vec![
                RepoNode {
                    worktrees: vec![worktree("feat/login", "/wt/app-login")],
                    ..only_repo()
                },
                RepoNode {
                    repo_key: "/src/site/.git".into(),
                    repo_root: "/src/site".into(),
                    display_name: "me/site".into(),
                    worktrees: vec![worktree("feat/login", "/wt/site-login")],
                },
            ],
            ungrouped: Vec::new(),
        };
        let trees = clean(&["/wt/app-login", "/wt/site-login"]);
        let judged = candidates(&tree, &facts(&trees, &asked(vec![merged(1, "feat/login")])));
        assert!(judged["/wt/app-login"].is_offered(), "its own repository");
        assert_eq!(
            judged["/wt/site-login"],
            Candidate::Available,
            "nobody asked gh about that repository at all"
        );
    }

    #[test]
    fn a_pull_request_offers_only_the_branch_it_was_actually_for() {
        // Not one whose name it merely contains. `login` and `feat/login` are two branches.
        let trees = clean(&["/wt/login"]);
        let judged = judged(
            &tree_of(vec![worktree("login", "/wt/login")]),
            &facts(&trees, &asked(vec![merged(5, "feat/login")])),
        );
        assert_eq!(judged["/wt/login"], Candidate::Available);
    }

    #[test]
    fn a_merged_pull_request_does_not_offer_a_checkout_holding_work() {
        // The twin of the `gone` rule, and the one that was unpinned: a working tree with
        // something in it is never offered, whatever GitHub says about the branch. git would
        // refuse the removal anyway — but the sweep's job is not to suggest what git will
        // refuse, it is to suggest what is finished with.
        for answer in [WorkingTree::Dirty, WorkingTree::Unreadable] {
            let trees = BTreeMap::from([("/wt/feat-login".to_string(), answer)]);
            let judged = judged(
                &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
                &facts(&trees, &asked(vec![merged(4, "feat/login")])),
            );
            assert_eq!(judged["/wt/feat-login"], Candidate::Available, "{answer:?}");
            assert!(
                judged["/wt/feat-login"].is_markable(),
                "still the user's to mark"
            );
        }
    }

    #[test]
    fn a_branch_that_landed_is_not_reported_by_whichever_pull_request_gh_listed_first() {
        // Closed, then reopened and merged — or a branch name used twice. `gh` answers
        // newest first, but nothing here pins that, and the row's reason must not turn on a
        // sort order. Merged wins: a branch with a merge behind it has landed, whatever else
        // also happened to it. Getting this the wrong way round tells someone their work was
        // abandoned as they delete the only copy of it.
        let trees = clean(&["/wt/feat-login"]);
        for order in [
            vec![
                settled(1, "feat/login", PullRequestOutcome::Closed),
                settled(2, "feat/login", PullRequestOutcome::Merged),
            ],
            vec![
                settled(2, "feat/login", PullRequestOutcome::Merged),
                settled(1, "feat/login", PullRequestOutcome::Closed),
            ],
        ] {
            let judged = judged(
                &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
                &facts(&trees, &asked(order)),
            );
            assert_eq!(judged["/wt/feat-login"].label_for_test(), "PR #2 merged");
        }
    }

    #[test]
    fn a_merge_wins_over_a_closure_that_came_after_it() {
        // The half the ordering test cannot see: here the merge is the *older* entry, so
        // preferring the later number alone would report the closure. The branch landed;
        // whatever happened to a later pull request for the same name did not unland it.
        let trees = clean(&["/wt/feat-login"]);
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(
                &trees,
                &asked(vec![
                    settled(2, "feat/login", PullRequestOutcome::Merged),
                    settled(9, "feat/login", PullRequestOutcome::Closed),
                ]),
            ),
        );
        assert_eq!(judged["/wt/feat-login"].label_for_test(), "PR #2 merged");
    }

    #[test]
    fn two_of_a_kind_are_reported_by_the_later_one() {
        // Nothing distinguishes them but which came second, and the second is the one whose
        // story the branch is at the end of.
        let trees = clean(&["/wt/feat-login"]);
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(
                &trees,
                &asked(vec![
                    settled(9, "feat/login", PullRequestOutcome::Closed),
                    settled(3, "feat/login", PullRequestOutcome::Closed),
                ]),
            ),
        );
        assert_eq!(judged["/wt/feat-login"].label_for_test(), "PR #9 closed");
    }

    #[test]
    fn a_branch_of_the_same_name_on_somebody_elses_fork_is_not_this_one() {
        // `gh` reports a fork's branch by its bare name, so a merged drive-by `patch-1`
        // arrives looking exactly like the local `patch-1` somebody is working on. On a
        // repository that takes contributions this is the everyday collision, and offering
        // it would be `gh` producing a wrong mark rather than widening a set.
        let trees = clean(&["/wt/patch-1"]);
        let from_a_fork = asked(vec![SettledPullRequest {
            number: 42,
            head_ref: "patch-1".to_string(),
            from_a_fork: true,
            outcome: PullRequestOutcome::Merged,
        }]);
        let judged = judged(
            &tree_of(vec![worktree("patch-1", "/wt/patch-1")]),
            &facts(&trees, &from_a_fork),
        );
        assert_eq!(judged["/wt/patch-1"], Candidate::Available);
    }

    #[test]
    fn a_row_git_would_refuse_anyway_is_not_called_unjudged() {
        // `gh` failing is only worth saying where its answer could have changed something.
        // A working tree holding work was never going to be offered whatever GitHub said.
        let dirty = BTreeMap::from([("/wt/feat-login".to_string(), WorkingTree::Dirty)]);
        let unavailable = BTreeMap::from([(RepoRoot::of(&only_repo()), None)]);
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&dirty, &unavailable),
        );
        assert_eq!(judged["/wt/feat-login"], Candidate::Available);
    }

    #[test]
    fn a_row_git_would_refuse_anyway_is_not_called_unjudged_by_a_full_window_either() {
        // The same rule down the other road. `gh` answered — it just may not have reached
        // back far enough — and the row it did not reach was one no pull request was going
        // to decide. On a repository with more than a window of closed pull requests this is
        // every row nobody has answered for yet, which is all of them on the first frame:
        // `Unjudged` there buries the rows that genuinely could not be judged.
        let holding_work = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(
                &BTreeMap::from([("/wt/feat-login".to_string(), WorkingTree::Dirty)]),
                &told(vec![merged(4, "something/else")], false),
            ),
        );
        assert_eq!(holding_work["/wt/feat-login"], Candidate::Available);

        let nothing_answered = BTreeMap::new();
        let unanswered = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&nothing_answered, &told(vec![], false)),
        );
        assert_eq!(
            unanswered["/wt/feat-login"],
            Candidate::Available,
            "a checkout git has not answered for yet is not a checkout gh failed on"
        );
    }

    #[test]
    fn which_refusal_a_checkout_that_earns_two_of_them_shows() {
        // Every other test here gives a checkout exactly one refusal, so the order among
        // them cancels out — and the order is the whole of what `Refusal::label` says. Most
        // permanent first: the repository itself never stops being that, a removal ends, and
        // panes close whenever somebody closes them.
        let mut all_three = worktree("main", "/src/app");
        all_three.is_primary = true;
        all_three.panes = vec![PaneNode {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            display_name: None,
            agent_status: AgentStatus::Idle,
            focused: false,
        }];

        let none = BTreeMap::new();
        let judged = candidates(
            &tree_of(vec![all_three]),
            &Facts {
                working_trees: &clean(&["/src/app"]),
                settled: &none,
                removing: &["/src/app".to_string()],
            },
        );
        assert_eq!(judged["/src/app"], Candidate::Refused(Refusal::Primary));
    }

    #[test]
    fn the_map_is_keyed_by_the_root_and_not_the_directory_beside_it() {
        // `RepoNode` carries `/src/app/.git` and `/src/app` side by side, and keying on the
        // wrong one answers nothing for every checkout in the tree: no marks, no `Unjudged`,
        // no error, nothing on screen. Every other test here builds both sides of the map
        // with `RepoRoot::of`, so the choice cancels out and only this pins it.
        assert_eq!(RepoRoot::of(&only_repo()), RepoRoot("/src/app".to_string()));
    }

    #[test]
    fn what_the_sweep_marks_and_what_the_user_may_mark_are_different_questions() {
        assert!(Candidate::Offered(Reason::Gone).is_offered());
        for own in [Candidate::Unjudged, Candidate::Available] {
            assert!(!own.is_offered(), "{own:?} is not marked for the user");
            assert!(own.is_markable(), "{own:?} is still the user's to mark");
        }
        for refusal in [Refusal::Primary, Refusal::Running, Refusal::Removing] {
            let refused = Candidate::Refused(refusal);
            assert!(!refused.is_offered());
            assert!(
                !refused.is_markable(),
                "{refusal:?} is nobody's to overrule"
            );
        }
    }

    #[test]
    fn every_refusal_says_which_one_it_is() {
        // A row that simply cannot be marked, with no word for why, reads as a bug.
        assert_eq!(Refusal::Primary.label(), "the repository itself");
        assert_eq!(Refusal::Running.label(), "panes are running in it");
        assert_eq!(Refusal::Removing.label(), "already being removed");
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
