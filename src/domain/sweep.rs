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
    /// a sentence for the prompt line, which the picker composes from [`Mark::refused`].
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

    /// Whether a checkout is refused outright, which is different from being left unmarked.
    ///
    /// The reason is carried rather than read out here: why `Space` did nothing is a
    /// sentence, and a sentence is [`ui::words`](crate::ui::words)'s.
    pub fn refused(&self) -> Option<Refusal> {
        match self {
            Mark::Refused(refusal) => Some(*refusal),
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
mod tests;
