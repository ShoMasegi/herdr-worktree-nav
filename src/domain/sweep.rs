//! Which checkouts a sweep may offer to delete, and why.
//!
//! `Shift-D` acts on the row under the cursor because a person put the cursor there; a sweep
//! has to decide for itself. The decision is `docs/adr/0011-what-may-be-swept.md`, and this
//! carries it out pure, where it can be read and tested without a git or a `gh`.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::model::{CheckoutPath, RepoKey, RepoNode, Tree, WorkingTree, WorktreeNode};
use crate::port::{PullRequestOutcome, SettledPullRequest, SettledPullRequests, Track};

/// Why a checkout is offered for deletion. The row shows it beside the mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// git cannot find the ref this branch tracks — the ordinary end of a branch whose pull
    /// request was merged and whose head the remote then deleted.
    Gone,
    /// The branch's pull request is finished with. Reached wherever git did not already
    /// offer the row, `Ahead`, `Behind` and `Diverged` included, so a branch carrying commits
    /// its upstream has not got can be offered on a merged pull request. The `git branch -d`
    /// that follows refuses an unmerged branch, so those commits outlive the sweep.
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
    /// Its removal is already going, in a process of its own. Never read out: the cursor does
    /// not stop on a row being removed, sweep or no sweep — see
    /// [`domain::rows::Row::is_selectable`](crate::domain::rows::Row::is_selectable). The
    /// variant still keeps the row out of [`chosen`].
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

/// Which half of the question nobody could answer.
///
/// A sweep asks two things of a checkout — git, whether its upstream is gone; `gh`, whether
/// its pull request is finished with — and the two are fixed in different places, so a row
/// neither could settle says which one went unanswered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    /// git would not read the repository's refs, so `gone` was never on offer — for every
    /// checkout of the repository at once. Named ahead of `gh` when both are missing: it is
    /// true outside a sweep too, and the ahead, behind and `gone` markers are missing with it.
    Refs,
    /// `gh` could not settle it: could not be asked, asked and the window it was given came
    /// back full, or asked and answered in a state this does not know.
    PullRequests,
}

impl Half {
    /// What the row says beside its box.
    pub fn label(self) -> &'static str {
        match self {
            Half::Refs => "refs unreadable",
            Half::PullRequests => "PR unknown",
        }
    }
}

/// What a sweep may do with one checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidate {
    /// Marked when the sweep opens.
    Offered(Reason),
    /// Nothing found that says it should go, and one half of the question went unanswered.
    ///
    /// Only where the answer would have changed something: a clean checkout on a branch,
    /// which is what both halves are asked about. A dirty one, a detached one, a refused one
    /// and one whose working tree has not been read are rows nothing was going to offer
    /// anyway, so nothing about them is unknown that matters.
    ///
    /// git's half fails one way: the ref walk failed, and every checkout of the repository
    /// then has `track: None` — which is also what a branch level with its upstream has, so
    /// the row cannot tell and the repository carries the fact instead
    /// ([`domain::model::Refs`](crate::domain::model::Refs)).
    Unjudged(Half),
    /// Nothing says it should go, and `Space` still marks it.
    Available,
    /// Never swept.
    Refused(Refusal),
}

impl Candidate {
    /// Whether the sweep marks this when it opens.
    pub fn is_offered(&self) -> bool {
        match self {
            Candidate::Offered(_) => true,
            Candidate::Unjudged(_) | Candidate::Available | Candidate::Refused(_) => false,
        }
    }

    /// Whether the user may mark it at all.
    ///
    /// Spelled out rather than written as `!matches!(_, Refused(_))`, which is the same
    /// answer today and would go on compiling as a variant was added — answering *yes* for
    /// it, because that is what the negation of a single pattern does. This is the predicate
    /// that decides what may be deleted, so a variant meant to be untouchable would reach a
    /// removal with nothing but [`chosen`] in the way. The compiler asks now.
    pub fn is_markable(&self) -> bool {
        match self {
            Candidate::Offered(_) | Candidate::Unjudged(_) | Candidate::Available => true,
            Candidate::Refused(_) => false,
        }
    }
}

/// Which repository a `gh` answer is about.
///
/// A newtype rather than a `String` because `RepoNode` carries two of those side by side —
/// `repo_key` (`/src/app/.git`) and `repo_root` (`/src/app`) — and a map keyed by the wrong
/// one silently answers nothing for every checkout in the tree: no marks, no `Unjudged`, no
/// error, nothing on screen. Outside this module [`RepoRoot::of`] is the only way to make
/// one; inside it a test pins the choice rather than the compiler.
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
    /// which is not clean — see
    /// [`domain::model::WorkingTree`](crate::domain::model::WorkingTree).
    pub working_trees: &'a BTreeMap<CheckoutPath, WorkingTree>,
    /// What `gh` said, by repository root. `None` for a repository `gh` could not be asked
    /// about — the reason belongs on the prompt line, not in a decision — and a repository
    /// absent from the map has not been asked at all.
    pub settled: &'a BTreeMap<RepoRoot, Option<SettledPullRequests>>,
    /// Checkout paths whose removal is already running.
    pub removing: &'a [CheckoutPath],
}

/// What the sweep may do with every checkout in the tree, by repository and checkout path.
pub fn candidates(tree: &Tree, facts: &Facts) -> BTreeMap<(RepoKey, CheckoutPath), Candidate> {
    let mut out = BTreeMap::new();
    for repo in &tree.repos {
        let settled = facts.settled.get(&RepoRoot::of(repo));
        for worktree in &repo.worktrees {
            let path = CheckoutPath::of(worktree);
            let candidate = judge(repo, worktree, &path, settled, facts);
            out.insert((RepoKey::of(repo), path), candidate);
        }
    }
    out
}

fn judge(
    repo: &RepoNode,
    worktree: &WorktreeNode,
    path: &CheckoutPath,
    settled: Option<&Option<SettledPullRequests>>,
    facts: &Facts,
) -> Candidate {
    // The refusals come first because they are about the checkout rather than about whether
    // anyone is finished with it. Their order among themselves decides only which refusal a
    // checkout that earns two of them carries, and it runs most permanent first: being the
    // repository itself is not a state that passes, a removal already going will end, and
    // panes close whenever somebody closes them.
    if worktree.is_primary {
        return Candidate::Refused(Refusal::Primary);
    }
    if facts.removing.contains(path) {
        return Candidate::Refused(Refusal::Removing);
    }
    if !worktree.panes.is_empty() {
        return Candidate::Refused(Refusal::Running);
    }

    // Every reason to offer below needs a working tree with nothing in it to lose. Clean is
    // a positive answer: dirty, unreadable and not-asked-yet are all "no", and marking one of
    // those by default would be deleting on the strength of a silence.
    let clean = facts
        .working_trees
        .get(path)
        .is_some_and(|answer| answer.is_clean());

    if clean && worktree.track == Some(Track::Gone) {
        return Candidate::Offered(Reason::Gone);
    }

    // Below here `gh` is the only thing left that could say anything, so a row it could not
    // reach is `Unjudged` rather than `Available` — but only where its answer would have
    // changed the outcome. The same gate holds for git's own half: `gone` was never going to
    // offer a dirty or a detached checkout.
    let could_have_decided = clean && worktree.branch.is_some();
    let refs_unread = could_have_decided && !repo.refs.is_read();
    let settled = match settled {
        // Nobody has asked `gh` yet: an answer is still on its way, and a permanent word on
        // a temporary state is what the working-tree walk already avoids. git's half is not
        // on its way — the refs are read before the first frame — so that half is said now.
        None if refs_unread => return Candidate::Unjudged(Half::Refs),
        None => return Candidate::Available,
        // Asked, and `gh` could not answer — unless git could not either, in which case the
        // half named is git's, since fixing that is what makes `gh`'s half worth reading.
        Some(None) if refs_unread => return Candidate::Unjudged(Half::Refs),
        Some(None) if could_have_decided => return Candidate::Unjudged(Half::PullRequests),
        Some(None) => return Candidate::Available,
        Some(Some(settled)) => settled,
    };

    // A hit and a miss are answered once per variant, because what a *miss* means is a fact
    // about the list rather than about the branch. A flag beside the list lets the two
    // meanings land in one arm; this shape cannot.
    let found = worktree
        .branch
        .as_ref()
        .and_then(|branch| finished_with(settled.pull_requests(), branch));
    match (found, settled) {
        // `gh` widens whatever git could or could not say: a finished pull request offers
        // the row even in a repository whose refs were not read.
        (Some(pull_request), _) if clean => Candidate::Offered(Reason::PullRequest {
            number: pull_request.number,
            outcome: pull_request.outcome,
        }),
        // Found, and the working tree has something in it: git would refuse the removal.
        (Some(_), _) => Candidate::Available,
        // Not found, and git never got to look for `gone`: nobody judged this row.
        (None, _) if refs_unread => Candidate::Unjudged(Half::Refs),
        // Missing from all of them: this branch has no finished pull request.
        (None, SettledPullRequests::All(_)) => Candidate::Available,
        // Missing from as many as `gh` was asked for: the window may not reach back far
        // enough, and "nothing to sweep" on the strength of a page size is a confident
        // wrong claim.
        (None, SettledPullRequests::Window(_)) if could_have_decided => {
            Candidate::Unjudged(Half::PullRequests)
        }
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
/// and taking whichever `gh` lists first turns the row's reason on a sort order nothing here
/// pins. Merged wins, because a branch with a merge behind it has landed whatever else also
/// happened to it; between two of a kind the later number is the later story.
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

/// What the user has said about the sweep's suggestions since it opened.
///
/// **The decision, not the keypress.** `Space` flips the row it is on, and what is written
/// down is where that left the row, not that it was pressed. A stored flip would be
/// exclusive-or-ed against a suggestion `judge` recomputes on every rebuild, so it would
/// change meaning underneath itself: mark a row `gh` says nothing about and let `gh` land and
/// agree, and the two cancel and the mark goes out; clear the `gone` row and let the walk
/// report it dirty, and a checkout the user said no to comes back marked.
///
/// Keyed by repository and checkout path rather than by row index, because the row list is
/// rebuilt underneath this every time a working tree answers. A path stays with the checkout
/// it names, and one whose checkout has left the tree stops matching anything.
#[derive(Debug, Default, Clone)]
pub struct Changes(BTreeMap<(RepoKey, CheckoutPath), Decision>);

/// One answer, and what it was about.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Decision {
    /// The branch the row carried when the user answered.
    ///
    /// A path outlives the checkout that had it: remove a worktree and make another in the
    /// same place — which `git worktree add` will do, and which a second herdr session can
    /// do while the picker is up — and the tree comes back through `replace_tree` with a
    /// different branch at a path the user has already said yes to, on the list `Enter` acts
    /// on.
    ///
    /// `None` is a detached checkout. Two of those at one path are as different as two
    /// branches are, and this cannot tell them apart: a `WorktreeNode` carries no commit, so
    /// an answer about one carries over to the other. What would close that is a commit id on
    /// the node, which nothing reads yet.
    branch: Option<String>,
    going: bool,
}

impl Changes {
    /// Flip one row, and answer whether it is now going.
    ///
    /// Takes the [`Mark`] rather than only the path so that a refusal cannot be flipped into
    /// a mark by a keypress. `None` is that refusal: nothing was recorded, and the caller has
    /// a sentence from [`Mark::refusal`] to put on the prompt line.
    pub fn flip(&mut self, repo: &RepoNode, worktree: &WorktreeNode, mark: &Mark) -> Option<bool> {
        if !mark.is_markable() {
            return None;
        }
        let going = !mark.is_going();
        self.0.insert(
            (RepoKey::of(repo), CheckoutPath::of(worktree)),
            Decision {
                branch: worktree.branch.clone(),
                going,
            },
        );
        Some(going)
    }

    /// The answers that are still about the checkouts they were given about.
    ///
    /// Everything the sweep decides on is worked out again on every rebuild, and this is the
    /// one input that is not — it is what the user said, and it is meant to outlive a `gh`
    /// answer landing and a working tree reporting. What it must not outlive is the checkout
    /// it was said about. A path is reused; an answer is not transferable.
    pub fn still_about(&self, tree: &Tree) -> Changes {
        let listed: BTreeMap<(RepoKey, CheckoutPath), Option<&str>> = tree
            .repos
            .iter()
            .flat_map(|repo| {
                repo.worktrees.iter().map(move |worktree| {
                    (
                        (RepoKey::of(repo), CheckoutPath::of(worktree)),
                        worktree.branch.as_deref(),
                    )
                })
            })
            .collect();
        Changes(
            self.0
                .iter()
                .filter(|(key, decision)| listed.get(key) == Some(&decision.branch.as_deref()))
                .map(|(key, decision)| (key.clone(), decision.clone()))
                .collect(),
        )
    }

    /// What the user said about this row, or `None` where they have said nothing.
    fn said(&self, key: &(RepoKey, CheckoutPath)) -> Option<bool> {
        self.0.get(key).map(|decision| decision.going)
    }
}

/// Which checkouts carry a mark right now, in repository and path order.
///
/// The sweep's suggestion for every row the user has not spoken about, and the user's own
/// answer for every row they have. This is the answer `Enter` acts on, so a refusal reaching
/// it would be a checkout deleted that the picker promised never to touch. It filters again
/// rather than trusting [`Changes::flip`], because a checkout somebody opens a pane in
/// becomes `Refused(Running)` the next time the tree is read, with the user's answer still
/// written down against it.
pub fn chosen<'a>(
    candidates: &'a BTreeMap<(RepoKey, CheckoutPath), Candidate>,
    changes: &Changes,
) -> BTreeSet<&'a (RepoKey, CheckoutPath)> {
    candidates
        .iter()
        .filter(|(key, candidate)| {
            candidate.is_markable() && changes.said(key).unwrap_or(candidate.is_offered())
        })
        .map(|(key, _)| key)
        .collect()
}

/// What one row shows while a sweep is on.
///
/// Everything a row needs and nothing a row has to work out for itself: the sweep's
/// suggestions and the user's changes are two collections, and a row that consulted both
/// would be a third place the rule lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// Going, for the reason shown: what the sweep found.
    Going(Reason),
    /// Going because the user said so, on a row the sweep had nothing to say about. The row
    /// says nothing beside the mark — "because you said so" is not a finding.
    GoingByHand,
    /// Going because the user said so, on a row neither git nor `gh` could settle. The row
    /// goes on saying which: it is the one row on the list where what is being acted on is
    /// nobody knows. Its own variant rather than a flag beside
    /// [`GoingByHand`](Mark::GoingByHand), because a mark carrying both a reason and "could
    /// not look" would be a state nothing here can mean.
    GoingUnjudged(Half),
    /// Staying, and the user may change that.
    Staying,
    /// Staying, and one half of the question went unanswered. The row says which.
    Unjudged(Half),
    /// Never swept, and why.
    Refused(Refusal),
}

impl Mark {
    /// Whether this row goes when the sweep runs.
    pub fn is_going(&self) -> bool {
        match self {
            Mark::Going(_) | Mark::GoingByHand | Mark::GoingUnjudged(_) => true,
            Mark::Staying | Mark::Unjudged(_) | Mark::Refused(_) => false,
        }
    }

    /// Whether `Space` does anything here.
    ///
    /// Exhaustive for the reason [`Candidate::is_markable`] is, and it has to agree with it:
    /// `Candidate` decides what may be deleted and this decides what the keyboard may touch,
    /// so a variant added to one has to be answered in both. Written as negations, both would
    /// say yes to a new variant and go on agreeing while both were wrong.
    pub fn is_markable(&self) -> bool {
        match self {
            Mark::Going(_)
            | Mark::GoingByHand
            | Mark::GoingUnjudged(_)
            | Mark::Staying
            | Mark::Unjudged(_) => true,
            Mark::Refused(_) => false,
        }
    }

    /// What the row says beside its mark, or nothing.
    ///
    /// [`Reason::Gone`] is left out: the marks the picker draws draws
    /// it as the branch's upstream marker on every row it is true of, and `judge` offers that
    /// reason only where `track` is `Gone`, so repeating it puts the same word on the row
    /// twice. (The converse does not hold: a row whose track is gone is not offered while it
    /// is primary, running, being removed, or not known to be clean.)
    ///
    /// A refusal is left out too: the absence of a box says it, and `Space` answers on the
    /// prompt line — see [`refusal`](Mark::refusal).
    pub fn note(&self) -> Option<String> {
        match self {
            Mark::Going(reason @ Reason::PullRequest { .. }) => Some(reason.label()),
            Mark::Unjudged(half) | Mark::GoingUnjudged(half) => Some(half.label().to_string()),
            Mark::Going(Reason::Gone) | Mark::GoingByHand | Mark::Staying | Mark::Refused(_) => {
                None
            }
        }
    }

    /// Why `Space` did nothing here, for the prompt line.
    ///
    /// On demand rather than on the row: the answer is only wanted by somebody who has just
    /// tried, and these are sentences rather than labels.
    pub fn refusal(&self) -> Option<&'static str> {
        match self {
            Mark::Refused(refusal) => Some(refusal.label()),
            _ => None,
        }
    }
}

/// What every checkout shows while a sweep is on, by repository and checkout path.
///
/// Worked out once per rebuild rather than per row: a row that recomputed it would be a
/// second implementation of the rule `chosen` deletes by, free to disagree with it.
pub fn marks(
    candidates: &BTreeMap<(RepoKey, CheckoutPath), Candidate>,
    changes: &Changes,
) -> BTreeMap<(RepoKey, CheckoutPath), Mark> {
    let going = chosen(candidates, changes);
    candidates
        .iter()
        .map(|(key, candidate)| {
            // Flat, so that every pairing of what the sweep found and what the user said is
            // an arm of its own. Nested, a wildcard on the inside answers for a new
            // `Candidate` with a box and nothing beside it, and the compiler stops only at
            // the outer arm.
            let going = going.contains(key);
            let mark = match candidate {
                Candidate::Refused(refusal) => Mark::Refused(*refusal),
                Candidate::Offered(reason) if going => Mark::Going(reason.clone()),
                Candidate::Available if going => Mark::GoingByHand,
                Candidate::Unjudged(half) if going => Mark::GoingUnjudged(*half),
                Candidate::Unjudged(half) => Mark::Unjudged(*half),
                // Including an `Offered` the user has just cleared: the sweep's reason is no
                // longer why this row is doing anything, so it stops being shown.
                Candidate::Offered(_) | Candidate::Available => Mark::Staying,
            };
            (key.clone(), mark)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{PaneNode, Refs, RepoNode, WorktreeNode};
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
            refs: Refs::Read,
            worktrees: Vec::new(),
        }
    }

    /// The same repository, with a ref walk git would not do.
    fn tree_unread(worktrees: Vec<WorktreeNode>) -> Tree {
        let mut tree = tree_of(worktrees);
        tree.repos[0].refs = Refs::Unreadable("fatal: bad ref".to_string());
        tree
    }

    /// The key a checkout of the one repository is judged under.
    fn at(path: &str) -> (RepoKey, CheckoutPath) {
        (RepoKey::of(&only_repo()), CheckoutPath::for_test(path))
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

    fn clean(paths: &[&str]) -> BTreeMap<CheckoutPath, WorkingTree> {
        paths
            .iter()
            .map(|path| (CheckoutPath::for_test(path), WorkingTree::Clean))
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

    fn judged(tree: &Tree, facts: &Facts) -> BTreeMap<(RepoKey, CheckoutPath), Candidate> {
        candidates(tree, facts)
    }

    /// The everything-is-fine case: one clean checkout, nothing running, nobody asked `gh`.
    fn facts<'a>(
        working_trees: &'a BTreeMap<CheckoutPath, WorkingTree>,
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
        assert_eq!(
            judged[&at("/wt/fix-crash")],
            Candidate::Offered(Reason::Gone)
        );
        assert_eq!(judged[&at("/wt/fix-crash")].label_for_test(), "gone");
    }

    #[test]
    fn nothing_is_offered_on_a_working_tree_nobody_has_answered_for() {
        // The state the picker opens in. Offering here deletes a checkout because a walk has
        // not finished yet, which waiting a moment longer cannot undo.
        let mut wt = worktree("fix/crash", "/wt/fix-crash");
        wt.track = Some(Track::Gone);
        let nothing = BTreeMap::new();
        let none = BTreeMap::new();
        let judged = judged(&tree_of(vec![wt]), &facts(&nothing, &none));
        assert_eq!(judged[&at("/wt/fix-crash")], Candidate::Available);
    }

    #[test]
    fn a_working_tree_git_would_not_read_is_never_offered() {
        // Not the same as reading it and finding nothing. `safe.directory`, or a checkout
        // whose directory has gone: git said it could not look, and offering on that offers
        // to delete whatever is in there on the strength of a failed question.
        let mut wt = worktree("fix/crash", "/wt/fix-crash");
        wt.track = Some(Track::Gone);
        let unreadable = BTreeMap::from([(
            CheckoutPath::for_test("/wt/fix-crash"),
            WorkingTree::Unreadable,
        )]);
        let none = BTreeMap::new();
        let judged = judged(&tree_of(vec![wt]), &facts(&unreadable, &none));
        assert_eq!(judged[&at("/wt/fix-crash")], Candidate::Available);
    }

    #[test]
    fn a_detached_checkout_is_not_called_unjudged_when_gh_could_not_be_asked() {
        // Saying "PR unknown" would blame `gh` for a silence git is responsible for.
        let mut wt = worktree("feat/login", "/wt/detached");
        wt.branch = None;
        let trees = clean(&["/wt/detached"]);
        let unavailable = BTreeMap::from([(RepoRoot::of(&only_repo()), None)]);
        let judged = judged(&tree_of(vec![wt]), &facts(&trees, &unavailable));
        assert_eq!(judged[&at("/wt/detached")], Candidate::Available);
    }

    #[test]
    fn a_working_tree_holding_work_is_not_offered_but_can_still_be_marked() {
        // git refuses the removal, and says what would have been lost.
        let mut wt = worktree("fix/crash", "/wt/fix-crash");
        wt.track = Some(Track::Gone);
        let dirty = BTreeMap::from([(CheckoutPath::for_test("/wt/fix-crash"), WorkingTree::Dirty)]);
        let none = BTreeMap::new();
        let judged = judged(&tree_of(vec![wt]), &facts(&dirty, &none));
        assert_eq!(judged[&at("/wt/fix-crash")], Candidate::Available);
        assert!(judged[&at("/wt/fix-crash")].is_markable());
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
                removing: &[CheckoutPath::for_test("/wt/fix-crash")],
            },
        );

        // Every one of them is clean with a gone upstream, so only the refusal keeps them out.
        assert_eq!(
            judged[&at("/src/app")],
            Candidate::Refused(Refusal::Primary)
        );
        assert_eq!(
            judged[&at("/wt/feat-login")],
            Candidate::Refused(Refusal::Running)
        );
        assert_eq!(
            judged[&at("/wt/fix-crash")],
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
            judged[&at("/wt/feat-login")],
            Candidate::Offered(Reason::PullRequest {
                number: 123,
                outcome: PullRequestOutcome::Merged,
            })
        );
        assert_eq!(
            judged[&at("/wt/feat-login")].label_for_test(),
            "PR #123 merged"
        );
    }

    #[test]
    fn gh_widens_and_never_overrides() {
        // A branch git already called `gone` keeps git's reason even where a pull request
        // would have given another.
        let mut wt = worktree("fix/crash", "/wt/fix-crash");
        wt.track = Some(Track::Gone);
        let trees = clean(&["/wt/fix-crash"]);
        let settled = asked(vec![merged(7, "fix/crash")]);
        let judged = judged(&tree_of(vec![wt]), &facts(&trees, &settled));
        assert_eq!(
            judged[&at("/wt/fix-crash")],
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
        assert_eq!(
            judged[&at("/wt/feat-login")],
            Candidate::Unjudged(Half::PullRequests)
        );
        assert_eq!(
            judged[&at("/wt/fix-crash")],
            Candidate::Offered(Reason::Gone),
            "git answered this one, so there was nothing to be unsure about"
        );
        assert_eq!(
            judged[&at("/wt/tidy")],
            Candidate::Refused(Refusal::Running),
            "a pull request was never going to decide this one"
        );
    }

    #[test]
    fn a_repository_whose_refs_git_would_not_read_says_so_on_the_rows_git_would_have_judged() {
        // git's half of ADR 0011's price. A failed ref walk leaves every checkout with
        // `track: None`, which is what a branch level with its upstream has too, so without
        // this the repository looks like one with nothing to find. The gate is the one
        // `gh`'s half is under: a dirty or a detached checkout is never offered on `gone`.
        let judgeable = worktree("feat/login", "/wt/feat-login");
        let holding_work = worktree("fix/crash", "/wt/fix-crash");
        let mut detached = worktree("", "/wt/detached");
        detached.branch = None;
        let mut running = worktree("chore/tidy", "/wt/tidy");
        running.panes = vec![PaneNode {
            pane_id: "w3:p1".into(),
            workspace_id: "w3".into(),
            tab_id: "w3:t1".into(),
            display_name: None,
            agent_status: AgentStatus::Idle,
            focused: false,
        }];
        let mut trees = clean(&["/wt/feat-login", "/wt/detached", "/wt/tidy"]);
        trees.insert(CheckoutPath::for_test("/wt/fix-crash"), WorkingTree::Dirty);
        let not_asked = BTreeMap::new();

        let judged = judged(
            &tree_unread(vec![judgeable, holding_work, detached, running]),
            &facts(&trees, &not_asked),
        );
        assert_eq!(
            judged[&at("/wt/feat-login")],
            Candidate::Unjudged(Half::Refs)
        );
        assert_eq!(
            judged[&at("/wt/fix-crash")],
            Candidate::Available,
            "git would refuse it anyway, so nothing unknown about it matters"
        );
        assert_eq!(
            judged[&at("/wt/detached")],
            Candidate::Available,
            "nothing to be gone from"
        );
        assert_eq!(
            judged[&at("/wt/tidy")],
            Candidate::Refused(Refusal::Running)
        );
    }

    #[test]
    fn gh_still_widens_a_repository_whose_refs_git_would_not_read() {
        // A failed ref walk takes nothing away from `gh`: a finished pull request offers the
        // row exactly as it would otherwise. Where `gh` has nothing to offer — no pull
        // request, or no answer — the half the row names is git's.
        let trees = clean(&["/wt/feat-login", "/wt/fix-crash"]);
        let both = vec![
            worktree("feat/login", "/wt/feat-login"),
            worktree("fix/crash", "/wt/fix-crash"),
        ];

        let found = judged(
            &tree_unread(both.clone()),
            &facts(&trees, &asked(vec![merged(7, "feat/login")])),
        );
        assert_eq!(
            found[&at("/wt/feat-login")],
            Candidate::Offered(Reason::PullRequest {
                number: 7,
                outcome: PullRequestOutcome::Merged,
            })
        );
        assert_eq!(
            found[&at("/wt/fix-crash")],
            Candidate::Unjudged(Half::Refs),
            "the whole list, and not in it — which is an answer from gh and none from git"
        );

        let unavailable = BTreeMap::from([(RepoRoot::of(&only_repo()), None)]);
        let neither = judged(&tree_unread(both.clone()), &facts(&trees, &unavailable));
        assert_eq!(
            neither[&at("/wt/feat-login")],
            Candidate::Unjudged(Half::Refs),
            "both halves missing: git's is the one named"
        );

        let window = judged(
            &tree_unread(both.clone()),
            &facts(&trees, &told(vec![merged(1, "some/other")], false)),
        );
        assert_eq!(
            window[&at("/wt/feat-login")],
            Candidate::Unjudged(Half::Refs)
        );

        // And a window that does reach the branch offers it, whatever git could not read.
        // Pinned separately because the arms are separate, and a guard on the wrong one
        // would leave this reading `Unjudged`.
        let reached = judged(
            &tree_unread(both),
            &facts(&trees, &told(vec![merged(7, "feat/login")], false)),
        );
        assert_eq!(
            reached[&at("/wt/feat-login")],
            Candidate::Offered(Reason::PullRequest {
                number: 7,
                outcome: PullRequestOutcome::Merged,
            })
        );
    }

    #[test]
    fn asked_and_told_nothing_is_not_the_same_as_not_being_able_to_ask() {
        // `Some(vec![])` against `None`: an answer that found nothing, against no answer.
        let trees = clean(&["/wt/feat-login"]);
        let answered = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &asked(Vec::new())),
        );
        assert_eq!(answered[&at("/wt/feat-login")], Candidate::Available);

        let unavailable = BTreeMap::from([(RepoRoot::of(&only_repo()), None)]);
        let could_not = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &unavailable),
        );
        assert_eq!(
            could_not[&at("/wt/feat-login")],
            Candidate::Unjudged(Half::PullRequests)
        );
    }

    #[test]
    fn a_branch_beyond_the_window_gh_was_given_is_not_called_finished_with() {
        // `gh` answers newest first and says nothing when it truncates, so "not in this
        // list" from a full window is not "no pull request".
        let trees = clean(&["/wt/feat-login"]);
        let truncated = told(vec![merged(1, "some/other")], false);
        let partial = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &truncated),
        );
        assert_eq!(
            partial[&at("/wt/feat-login")],
            Candidate::Unjudged(Half::PullRequests)
        );

        // And the same list, known to be all of them, is an answer.
        let whole = told(vec![merged(1, "some/other")], true);
        let complete = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &whole),
        );
        assert_eq!(complete[&at("/wt/feat-login")], Candidate::Available);
    }

    #[test]
    fn a_branch_found_in_a_truncated_window_is_still_an_answer() {
        // Truncation only casts doubt on absence.
        let trees = clean(&["/wt/feat-login"]);
        let truncated = told(vec![merged(4, "feat/login")], false);
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &truncated),
        );
        assert_eq!(
            judged[&at("/wt/feat-login")],
            Candidate::Offered(Reason::PullRequest {
                number: 4,
                outcome: PullRequestOutcome::Merged,
            })
        );
    }

    #[test]
    fn a_repository_nobody_has_asked_gh_about_yet_is_not_reported_as_unseen() {
        // An answer is still on its way, and "PR unknown" is a permanent word on a temporary
        // state — the mistake the working-tree walk avoids by drawing no marker until it
        // knows.
        let trees = clean(&["/wt/feat-login"]);
        let none = BTreeMap::new();
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &none),
        );
        assert_eq!(judged[&at("/wt/feat-login")], Candidate::Available);
    }

    #[test]
    fn a_pull_request_that_was_closed_is_not_reported_as_one_that_landed() {
        // "PR #4 merged" says the work is in and "PR #4 closed" says it was abandoned, so
        // the wrong way round tells someone their work landed as they delete the only copy.
        let trees = clean(&["/wt/feat-login"]);
        let abandoned = asked(vec![settled(4, "feat/login", PullRequestOutcome::Closed)]);
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&trees, &abandoned),
        );
        assert_eq!(
            judged[&at("/wt/feat-login")].label_for_test(),
            "PR #4 closed"
        );
    }

    #[test]
    fn one_repositorys_pull_requests_never_judge_anothers_checkouts() {
        // `feat/login` in two repositories is ordinary, and only one of them was asked about.
        // Answering for both offers to delete a checkout on the strength of a merge that
        // happened somewhere else entirely.
        let tree = Tree {
            repos: vec![
                RepoNode {
                    refs: Refs::Read,
                    worktrees: vec![worktree("feat/login", "/wt/app-login")],
                    ..only_repo()
                },
                RepoNode {
                    repo_key: "/src/site/.git".into(),
                    repo_root: "/src/site".into(),
                    display_name: "me/site".into(),
                    refs: Refs::Read,
                    worktrees: vec![worktree("feat/login", "/wt/site-login")],
                },
            ],
            ungrouped: Vec::new(),
        };
        let trees = clean(&["/wt/app-login", "/wt/site-login"]);
        let judged = candidates(&tree, &facts(&trees, &asked(vec![merged(1, "feat/login")])));
        assert!(
            judged[&at("/wt/app-login")].is_offered(),
            "its own repository"
        );
        assert_eq!(
            judged[&(
                RepoKey::of(&tree.repos[1]),
                CheckoutPath::for_test("/wt/site-login")
            )],
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
        assert_eq!(judged[&at("/wt/login")], Candidate::Available);
    }

    #[test]
    fn a_merged_pull_request_does_not_offer_a_checkout_holding_work() {
        // The twin of the `gone` rule: a working tree with something in it is never offered,
        // whatever GitHub says about the branch.
        for answer in [WorkingTree::Dirty, WorkingTree::Unreadable] {
            let trees = BTreeMap::from([(CheckoutPath::for_test("/wt/feat-login"), answer)]);
            let judged = judged(
                &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
                &facts(&trees, &asked(vec![merged(4, "feat/login")])),
            );
            assert_eq!(
                judged[&at("/wt/feat-login")],
                Candidate::Available,
                "{answer:?}"
            );
            assert!(
                judged[&at("/wt/feat-login")].is_markable(),
                "still the user's to mark"
            );
        }
    }

    #[test]
    fn a_branch_that_landed_is_not_reported_by_whichever_pull_request_gh_listed_first() {
        // Closed, then reopened and merged — or a branch name used twice. `gh` answers newest
        // first, but nothing here pins that, and the wrong way round tells someone their work
        // was abandoned as they delete the only copy of it.
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
            assert_eq!(
                judged[&at("/wt/feat-login")].label_for_test(),
                "PR #2 merged"
            );
        }
    }

    #[test]
    fn a_merge_wins_over_a_closure_that_came_after_it() {
        // The half the ordering test cannot see: here the merge is the *older* entry, so
        // preferring the later number alone would report the closure.
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
        assert_eq!(
            judged[&at("/wt/feat-login")].label_for_test(),
            "PR #2 merged"
        );
    }

    #[test]
    fn two_of_a_kind_are_reported_by_the_later_one() {
        // Nothing distinguishes them but which came second.
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
        assert_eq!(
            judged[&at("/wt/feat-login")].label_for_test(),
            "PR #9 closed"
        );
    }

    #[test]
    fn a_branch_of_the_same_name_on_somebody_elses_fork_is_not_this_one() {
        // `gh` reports a fork's branch by its bare name, so a merged drive-by `patch-1`
        // arrives looking exactly like the local `patch-1` somebody is working on. On a
        // repository that takes contributions this is the everyday collision.
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
        assert_eq!(judged[&at("/wt/patch-1")], Candidate::Available);
    }

    #[test]
    fn a_row_git_would_refuse_anyway_is_not_called_unjudged() {
        // A working tree holding work is not offered whatever GitHub says about the branch.
        let dirty =
            BTreeMap::from([(CheckoutPath::for_test("/wt/feat-login"), WorkingTree::Dirty)]);
        let unavailable = BTreeMap::from([(RepoRoot::of(&only_repo()), None)]);
        let judged = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&dirty, &unavailable),
        );
        assert_eq!(judged[&at("/wt/feat-login")], Candidate::Available);
    }

    #[test]
    fn a_row_git_would_refuse_anyway_is_not_called_unjudged_by_a_full_window_either() {
        // On a repository with more than a window of closed pull requests this is every row
        // nobody has answered for yet, which is all of them on the first frame: `Unjudged`
        // there buries the rows that genuinely could not be judged.
        let holding_work = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(
                &BTreeMap::from([(CheckoutPath::for_test("/wt/feat-login"), WorkingTree::Dirty)]),
                &told(vec![merged(4, "something/else")], false),
            ),
        );
        assert_eq!(holding_work[&at("/wt/feat-login")], Candidate::Available);

        let nothing_answered = BTreeMap::new();
        let unanswered = judged(
            &tree_of(vec![worktree("feat/login", "/wt/feat-login")]),
            &facts(&nothing_answered, &told(vec![], false)),
        );
        assert_eq!(
            unanswered[&at("/wt/feat-login")],
            Candidate::Available,
            "a checkout git has not answered for yet is not a checkout gh failed on"
        );
    }

    #[test]
    fn which_refusal_a_checkout_that_earns_two_of_them_shows() {
        // Every other test here gives a checkout exactly one refusal, so the order among them
        // cancels out and only this pins it.
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
                removing: &[CheckoutPath::for_test("/src/app")],
            },
        );
        assert_eq!(
            judged[&at("/src/app")],
            Candidate::Refused(Refusal::Primary)
        );

        // And the pair below it, which asserting only the top of the order leaves free. The
        // other way round sends the user back to close panes that a removal already running
        // is about to take with it, and never says the removal is happening.
        let mut both = worktree("feat/login", "/wt/feat-login");
        both.panes = vec![PaneNode {
            pane_id: "w2:p1".into(),
            workspace_id: "w2".into(),
            tab_id: "w2:t1".into(),
            display_name: None,
            agent_status: AgentStatus::Idle,
            focused: false,
        }];
        let judged = candidates(
            &tree_of(vec![both]),
            &Facts {
                working_trees: &clean(&["/wt/feat-login"]),
                settled: &none,
                removing: &[CheckoutPath::for_test("/wt/feat-login")],
            },
        );
        assert_eq!(
            judged[&at("/wt/feat-login")],
            Candidate::Refused(Refusal::Removing)
        );
    }

    #[test]
    fn the_map_is_keyed_by_the_root_and_not_the_directory_beside_it() {
        // Every other test here builds both sides of the map with `RepoRoot::of`, so the
        // choice cancels out and only this pins it.
        assert_eq!(RepoRoot::of(&only_repo()), RepoRoot("/src/app".to_string()));
    }

    #[test]
    fn one_repositorys_judgement_never_lands_on_anothers_checkout_at_the_same_path() {
        // Issue #46: two repositories name one path — `me/old` a worktree it once had there,
        // `me/app` a live checkout whose refs git would not read. Keyed by the path alone,
        // the stale `Offered(Gone)` took the live row.
        let mut stale = worktree("chore/deps", "/wt/shared");
        stale.track = Some(Track::Gone);
        let tree = Tree {
            repos: vec![
                RepoNode {
                    refs: Refs::Unreadable("fatal: bad ref".to_string()),
                    worktrees: vec![worktree("feat/x", "/wt/shared")],
                    ..only_repo()
                },
                RepoNode {
                    repo_key: "/src/old/.git".into(),
                    repo_root: "/src/old".into(),
                    display_name: "me/old".into(),
                    refs: Refs::Read,
                    worktrees: vec![stale],
                },
            ],
            ungrouped: Vec::new(),
        };
        let trees = clean(&["/wt/shared"]);
        let none = BTreeMap::new();
        let judged = candidates(&tree, &facts(&trees, &none));
        assert_eq!(judged.len(), 2, "one entry per checkout, not one per path");
        assert_eq!(
            judged[&(
                RepoKey::of(&tree.repos[0]),
                CheckoutPath::of(&tree.repos[0].worktrees[0])
            )],
            Candidate::Unjudged(Half::Refs)
        );
        assert_eq!(
            judged[&(
                RepoKey::of(&tree.repos[1]),
                CheckoutPath::of(&tree.repos[1].worktrees[0])
            )],
            Candidate::Offered(Reason::Gone)
        );
    }

    /// What `Space` does, through the same two steps the picker takes: judge every row, then
    /// flip the one the cursor is on by what it shows.
    fn flip(
        changes: &mut Changes,
        candidates: &BTreeMap<(RepoKey, CheckoutPath), Candidate>,
        path: &str,
    ) -> Option<bool> {
        let shown = marks(candidates, changes);
        changes.flip(&only_repo(), &worktree("b", path), &shown[&at(path)])
    }

    /// The four answers side by side, since what `Space` does depends on which one a row is.
    fn one_of_each() -> BTreeMap<(RepoKey, CheckoutPath), Candidate> {
        BTreeMap::from([
            (at("/wt/gone"), Candidate::Offered(Reason::Gone)),
            (at("/wt/unjudged"), Candidate::Unjudged(Half::PullRequests)),
            (at("/wt/available"), Candidate::Available),
            (at("/src/app"), Candidate::Refused(Refusal::Primary)),
        ])
    }

    #[test]
    fn the_sweep_opens_on_what_it_offered_and_nothing_else() {
        let candidates = one_of_each();
        assert_eq!(
            chosen(&candidates, &Changes::default()),
            BTreeSet::from([&at("/wt/gone")])
        );
    }

    #[test]
    fn space_widens_the_sweep_and_narrows_it_with_the_same_key() {
        // A pair of add and remove keys would make one of the two the easier answer.
        let candidates = one_of_each();
        let mut changes = Changes::default();

        assert_eq!(
            flip(&mut changes, &candidates, "/wt/available"),
            Some(true),
            "a row the sweep said nothing about is now going"
        );
        assert_eq!(
            flip(&mut changes, &candidates, "/wt/gone"),
            Some(false),
            "and one it offered is not"
        );
        assert_eq!(
            chosen(&candidates, &changes),
            BTreeSet::from([&at("/wt/available")])
        );

        // And back: a user who marks the wrong row reaches for the key they just pressed.
        assert_eq!(
            flip(&mut changes, &candidates, "/wt/available"),
            Some(false)
        );
        assert_eq!(flip(&mut changes, &candidates, "/wt/gone"), Some(true));
        assert_eq!(
            chosen(&candidates, &changes),
            BTreeSet::from([&at("/wt/gone")])
        );
    }

    #[test]
    fn gh_agreeing_with_a_mark_the_user_made_does_not_take_it_away() {
        // A stored keypress is exclusive-or-ed against a suggestion that moves, so `gh`
        // arriving at the answer the user already gave would cancel it out.
        let mut candidates = one_of_each();
        let mut changes = Changes::default();
        flip(&mut changes, &candidates, "/wt/available");
        assert!(chosen(&candidates, &changes).contains(&at("/wt/available")));

        // `gh` answers, and the sweep now suggests what the user already said.
        candidates.insert(
            at("/wt/available"),
            Candidate::Offered(Reason::PullRequest {
                number: 7,
                outcome: PullRequestOutcome::Merged,
            }),
        );
        assert!(
            chosen(&candidates, &changes).contains(&at("/wt/available")),
            "gh agreeing with the user cannot take the user's mark away"
        );
    }

    #[test]
    fn a_row_the_user_cleared_does_not_come_back_when_the_facts_move() {
        // The dangerous direction: a checkout the user said no to, coming back marked while
        // they are not looking, on the list `Enter` will act on.
        let mut candidates = one_of_each();
        let mut changes = Changes::default();
        flip(&mut changes, &candidates, "/wt/gone");
        assert!(!chosen(&candidates, &changes).contains(&at("/wt/gone")));

        // The walk answers: that working tree is dirty, so the sweep stops offering it.
        candidates.insert(at("/wt/gone"), Candidate::Available);
        assert!(
            !chosen(&candidates, &changes).contains(&at("/wt/gone")),
            "a checkout the user said no to does not come back marked"
        );
    }

    #[test]
    fn a_row_the_sweep_refuses_is_not_the_users_to_overrule() {
        // `Shift-D` asks before it removes and a sweep does not, so a refusal that could be
        // flipped into a mark is the repository's own checkout gone on two keys and no
        // question.
        let candidates = one_of_each();
        let mut changes = Changes::default();
        assert_eq!(flip(&mut changes, &candidates, "/src/app"), None);
        assert!(
            !chosen(&candidates, &changes).contains(&at("/src/app")),
            "and nothing was recorded to leak into the answer"
        );
    }

    #[test]
    fn an_answer_is_about_the_checkout_it_was_given_about_and_not_about_the_path() {
        // Remove a worktree and make another in the same place — `git worktree add` will,
        // and a second herdr session can while the picker is up — and that yes is about a
        // branch nobody has seen, on the list `Enter` acts on.
        let tree = tree_of(vec![worktree("feat/login", "/wt/feat-login")]);
        let candidates = BTreeMap::from([(at("/wt/feat-login"), Candidate::Available)]);
        let mut changes = Changes::default();
        changes.flip(
            &only_repo(),
            &worktree("feat/login", "/wt/feat-login"),
            &Mark::Staying,
        );
        assert!(chosen(&candidates, &changes.still_about(&tree)).contains(&at("/wt/feat-login")));

        // Same path, different branch.
        let moved = tree_of(vec![worktree("release/v2", "/wt/feat-login")]);
        assert!(
            !chosen(&candidates, &changes.still_about(&moved)).contains(&at("/wt/feat-login")),
            "the answer was about feat/login, and feat/login is not there any more"
        );

        // And a checkout that leaves the tree and comes back the same keeps its answer.
        assert!(chosen(&candidates, &changes.still_about(&tree)).contains(&at("/wt/feat-login")));
    }

    #[test]
    fn a_detached_checkout_is_not_the_same_checkout_as_a_branch_at_the_same_path() {
        // Two checkouts with no branch at one path are as different as two branches are.
        let mut detached = worktree("feat/login", "/wt/feat-login");
        detached.branch = None;
        let candidates = BTreeMap::from([(at("/wt/feat-login"), Candidate::Available)]);

        let mut changes = Changes::default();
        changes.flip(&only_repo(), &detached, &Mark::Staying);
        assert!(chosen(
            &candidates,
            &changes.still_about(&tree_of(vec![detached.clone()]))
        )
        .contains(&at("/wt/feat-login")));
        assert!(
            !chosen(
                &candidates,
                &changes.still_about(&tree_of(vec![worktree("feat/login", "/wt/feat-login")]))
            )
            .contains(&at("/wt/feat-login")),
            "a branch was checked out where there was none"
        );
    }

    #[test]
    fn an_answer_about_a_checkout_that_left_the_tree_is_not_kept() {
        let mut changes = Changes::default();
        changes.flip(
            &only_repo(),
            &worktree("feat/login", "/wt/feat-login"),
            &Mark::Staying,
        );
        assert!(changes.said(&at("/wt/feat-login")).is_some());

        let empty = tree_of(Vec::new());
        assert!(changes
            .still_about(&empty)
            .said(&at("/wt/feat-login"))
            .is_none());
    }

    #[test]
    fn a_row_that_becomes_a_refusal_under_a_mark_stops_being_chosen() {
        // A checkout somebody opens a pane in between two frames becomes `Refused(Running)`
        // while the user's flip is still recorded against it, and filtering only on the way
        // in would leave that mark standing on a row the sweep may not touch.
        let mut candidates = one_of_each();
        let mut changes = Changes::default();
        flip(&mut changes, &candidates, "/wt/available");
        assert!(chosen(&candidates, &changes).contains(&at("/wt/available")));

        candidates.insert(at("/wt/available"), Candidate::Refused(Refusal::Running));
        assert!(
            !chosen(&candidates, &changes).contains(&at("/wt/available")),
            "somebody started working in it; the mark goes with the judgement"
        );
    }

    #[test]
    fn a_flip_is_remembered_against_a_checkout_and_not_against_a_row() {
        // The list is rebuilt underneath the sweep every time a working tree answers, so a
        // remembered row index would end up on a different checkout.
        let mut candidates = one_of_each();
        let mut changes = Changes::default();
        flip(&mut changes, &candidates, "/wt/available");

        candidates.remove(&at("/wt/available"));
        assert_eq!(
            chosen(&candidates, &changes),
            BTreeSet::from([&at("/wt/gone")]),
            "the flip left with the checkout it named, and the rest is untouched"
        );

        // And a checkout made afterwards does not inherit it by sorting into the same place.
        candidates.insert(at("/wt/available"), Candidate::Available);
        candidates.insert(at("/wt/aaa-first"), Candidate::Available);
        assert_eq!(
            chosen(&candidates, &changes),
            BTreeSet::from([&at("/wt/gone"), &at("/wt/available")]),
            "though a path used again is the one thing a path cannot tell apart"
        );
    }

    #[test]
    fn every_row_says_what_is_happening_to_it_and_why() {
        let candidates = one_of_each();
        let shown = marks(&candidates, &Changes::default());

        assert_eq!(
            shown[&at("/wt/gone")],
            Mark::Going(Reason::Gone),
            "nothing goes without its reason attached"
        );
        assert_eq!(
            shown[&at("/wt/gone")].note(),
            None,
            "and its reason is the upstream marker the row already draws — `judge` offers \
             `Gone` only where the track is gone, so saying it again would put the same \
             word on the row twice"
        );
        assert_eq!(shown[&at("/wt/available")], Mark::Staying);
        assert_eq!(shown[&at("/wt/available")].note(), None);
        assert_eq!(
            shown[&at("/wt/unjudged")],
            Mark::Unjudged(Half::PullRequests)
        );
        assert_eq!(
            shown[&at("/wt/unjudged")].note().as_deref(),
            Some("PR unknown"),
            "a row gh could not judge says so rather than looking like one with nothing to \
             find"
        );
        assert_eq!(shown[&at("/src/app")], Mark::Refused(Refusal::Primary));
        assert_eq!(
            shown[&at("/src/app")].note(),
            None,
            "a refusal is said by the absence of a box, not by a sentence where the label goes"
        );
        assert_eq!(
            shown[&at("/src/app")].refusal(),
            Some("the repository itself"),
            "and answered on the prompt line to whoever pressed Space"
        );
        assert_eq!(
            shown[&at("/wt/gone")].refusal(),
            None,
            "nothing else has one to give"
        );
    }

    #[test]
    fn the_reason_a_gone_branch_is_going_is_the_marker_the_row_already_carries() {
        // `Mark::note` stays quiet here, which is only right while `judge` offers `Gone` for
        // exactly the rows the marks the picker draws draws `gone` on. If that comes apart, a row
        // goes with nothing on it saying why.
        let mut worktree = worktree("feat/login", "/wt/feat-login");
        worktree.track = Some(Track::Gone);
        let judged = judged(
            &tree_of(vec![worktree]),
            &facts(&clean(&["/wt/feat-login"]), &BTreeMap::new()),
        );
        assert_eq!(
            judged[&at("/wt/feat-login")],
            Candidate::Offered(Reason::Gone),
            "gone is offered for a track that is gone, and for nothing else"
        );
    }

    #[test]
    fn a_pull_request_is_named_on_the_row_it_decided() {
        let candidates = BTreeMap::from([(
            at("/wt/feat-login"),
            Candidate::Offered(Reason::PullRequest {
                number: 123,
                outcome: PullRequestOutcome::Merged,
            }),
        )]);
        let shown = marks(&candidates, &Changes::default());
        assert_eq!(
            shown[&at("/wt/feat-login")].note().as_deref(),
            Some("PR #123 merged"),
            "the number is what makes the reason checkable"
        );
    }

    #[test]
    fn a_row_the_user_marked_says_nothing_beside_its_mark() {
        // A note there would make the sweep look as though it had agreed.
        let candidates = one_of_each();
        let mut changes = Changes::default();
        flip(&mut changes, &candidates, "/wt/available");

        let shown = marks(&candidates, &changes);
        assert_eq!(shown[&at("/wt/available")], Mark::GoingByHand);
        assert_eq!(shown[&at("/wt/available")].note(), None);
    }

    #[test]
    fn a_row_marked_where_gh_could_not_look_goes_on_saying_so() {
        // The one exception to the row above, and the moment it starts to matter: without
        // it, a box on a row nobody could judge reads exactly like a box on a row `gh`
        // answered for, on the list `Enter` will act on.
        let candidates = one_of_each();
        let mut changes = Changes::default();
        flip(&mut changes, &candidates, "/wt/unjudged");

        let shown = marks(&candidates, &changes);
        assert_eq!(
            shown[&at("/wt/unjudged")],
            Mark::GoingUnjudged(Half::PullRequests)
        );
        assert!(shown[&at("/wt/unjudged")].is_going());
        assert_eq!(
            shown[&at("/wt/unjudged")].note().as_deref(),
            Some("PR unknown")
        );

        // And it comes off again: a variant left out of `is_markable`'s `true` arm is a mark
        // the user can put on and never take off.
        flip(&mut changes, &candidates, "/wt/unjudged");
        let shown = marks(&candidates, &changes);
        assert_eq!(
            shown[&at("/wt/unjudged")],
            Mark::Unjudged(Half::PullRequests)
        );
    }

    #[test]
    fn a_row_says_which_half_of_the_question_went_unanswered() {
        // The two halves are fixed in different places, so the row says which one to go to.
        let candidates = BTreeMap::from([
            (at("/wt/refs"), Candidate::Unjudged(Half::Refs)),
            (at("/wt/prs"), Candidate::Unjudged(Half::PullRequests)),
        ]);
        let mut changes = Changes::default();
        let shown = marks(&candidates, &changes);
        assert_eq!(shown[&at("/wt/refs")], Mark::Unjudged(Half::Refs));
        assert_eq!(
            shown[&at("/wt/refs")].note().as_deref(),
            Some("refs unreadable")
        );
        assert_eq!(shown[&at("/wt/prs")].note().as_deref(), Some("PR unknown"));

        flip(&mut changes, &candidates, "/wt/refs");
        let shown = marks(&candidates, &changes);
        assert_eq!(shown[&at("/wt/refs")], Mark::GoingUnjudged(Half::Refs));
        assert_eq!(
            shown[&at("/wt/refs")].note().as_deref(),
            Some("refs unreadable"),
            "marked by hand, it still says nobody judged it — and which half"
        );
    }

    #[test]
    fn a_row_the_user_cleared_stops_showing_the_reason_it_was_offered_for() {
        // The reason is why the sweep would take it, and it no longer would.
        let candidates = one_of_each();
        let mut changes = Changes::default();
        flip(&mut changes, &candidates, "/wt/gone");

        let shown = marks(&candidates, &changes);
        assert_eq!(shown[&at("/wt/gone")], Mark::Staying);
        assert_eq!(shown[&at("/wt/gone")].note(), None);
    }

    #[test]
    fn what_a_row_shows_and_what_the_sweep_takes_are_the_same_answer() {
        // A row working this out for itself would be a second implementation of the rule,
        // free to disagree with the one that deletes things.
        let candidates = one_of_each();
        let mut changes = Changes::default();
        flip(&mut changes, &candidates, "/wt/available");
        flip(&mut changes, &candidates, "/wt/gone");

        let going: BTreeSet<(RepoKey, CheckoutPath)> = marks(&candidates, &changes)
            .into_iter()
            .filter(|(_, mark)| mark.is_going())
            .map(|(path, _)| path)
            .collect();
        let taken: BTreeSet<(RepoKey, CheckoutPath)> =
            chosen(&candidates, &changes).into_iter().cloned().collect();
        assert_eq!(going, taken);
    }

    #[test]
    fn space_is_refused_on_exactly_the_rows_that_show_a_refusal() {
        let candidates = one_of_each();
        for (key, mark) in marks(&candidates, &Changes::default()) {
            assert_eq!(
                mark.is_markable(),
                candidates[&key].is_markable(),
                "{key:?} disagrees with itself about whether Space does anything"
            );
        }
    }

    #[test]
    fn what_the_sweep_marks_and_what_the_user_may_mark_are_different_questions() {
        assert!(Candidate::Offered(Reason::Gone).is_offered());
        for own in [
            Candidate::Unjudged(Half::PullRequests),
            Candidate::Available,
        ] {
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
        // Nothing points at it, so there is no head ref for a pull request to match.
        let mut wt = worktree("feat/login", "/wt/detached");
        wt.branch = None;
        let trees = clean(&["/wt/detached"]);
        let settled = asked(vec![merged(9, "feat/login")]);
        let judged = judged(&tree_of(vec![wt]), &facts(&trees, &settled));
        assert_eq!(judged[&at("/wt/detached")], Candidate::Available);
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
