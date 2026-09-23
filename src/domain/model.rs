//! The model both pickers display: repositories, their worktrees, and the panes sitting in
//! each one.

use crate::port::{AgentStatus, Track};

/// A repository, identified the way herdr identifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoNode {
    /// Normalised git common directory. Shared by the repository and all its worktrees.
    pub repo_key: String,
    /// Top level of the primary checkout.
    pub repo_root: String,
    /// `owner/repo` when the origin is on GitHub, otherwise the directory name.
    pub display_name: String,
    pub worktrees: Vec<WorktreeNode>,
    pub refs: Refs,
}

/// What git said about a checkout's working tree.
///
/// Three answers, not a boolean, because a sweep decides on the difference. "Not dirty" is
/// what a boolean gives, and it is the wrong fact to delete a checkout on: it also means
/// "nobody has asked yet" and "git would not say". `Option<WorkingTree>` is the whole
/// picture, with `None` for a checkout still being walked —
/// `docs/adr/0011-what-may-be-swept.md` is where the distinction stops being cosmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingTree {
    /// git had nothing to report. The only one of these a sweep may act on.
    ///
    /// What git reports is not everything that is there: a path it was told to ignore, and a
    /// file under `assume-unchanged`, are outside the question — and outside the one
    /// `git worktree remove` asks too, so a sweep acting on this takes what deleting by hand
    /// would have taken. [`GitPort::is_dirty`](crate::port::GitPort::is_dirty) carries the
    /// whole of it.
    Clean,
    /// Modified tracked files, or untracked ones. The same question `git worktree remove`
    /// asks before it refuses.
    Dirty,
    /// git declined. Not the same claim as clean — and the one answer of the three that is
    /// about the tooling rather than the work. Nothing here tells its shapes apart: a
    /// `safe.directory` refusal fails identically for every checkout at once and a worktree
    /// whose directory has gone fails for exactly one, and both arrive as this. Whether that
    /// is worth a fourth variant is a question for whoever first has to explain one to a
    /// user.
    Unreadable,
}

impl WorkingTree {
    pub fn is_clean(self) -> bool {
        self == WorkingTree::Clean
    }

    /// Whether a row shows a mark for this. Clean draws nothing, and so does a checkout
    /// nobody has answered for yet — which is why the commonest transition of all does not
    /// rebuild the list.
    ///
    /// Spelled out rather than written as `!= Clean`, which is the same thing today and would
    /// go on compiling as an answer that draws nothing was added. This has to track
    /// the marks the picker draws. The compiler will make whoever
    /// adds a variant *visit* both, which `!= Clean` would not — it cannot make them agree, so
    /// the two still have to be read together.
    pub fn is_drawn(self) -> bool {
        match self {
            WorkingTree::Clean => false,
            WorkingTree::Dirty | WorkingTree::Unreadable => true,
        }
    }
}

/// Whether git read a repository's refs.
///
/// The markers a checkout carries — `↑2`, `↓1`, `gone` — come off one `for-each-ref` per
/// repository, so when that call fails every checkout in the repository loses them at once.
/// Each then carries none of the three, which is right: no marker beats a wrong one. What a bare
/// absence cannot do is tell "nothing to report" from "git would not say", and for `gone`
/// that is the difference between a repository with nothing to sweep and one nobody has
/// looked at — `docs/adr/0011-what-may-be-swept.md`. So the fact travels with the
/// repository, and the prompt line says it once rather than every row guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refs {
    /// git answered. A checkout here with no track marker has nothing git reported for it,
    /// or is one whose `:track` field git printed and the adapter could not read —
    /// [`Track::Unreadable`] — or is one more than one of the repository's refs names, which
    /// [`Position::Contested`] is. The three draw no marker for three reasons, and each
    /// carries its own.
    Read,
    /// git did not, in its own words — or, in a debug build only, the thread that asked
    /// did not finish.
    ///
    /// Every checkout here is missing its track markers. `domain::tree::tracks` is keyed by
    /// repository as well as by checkout path, so no other repository's answer reaches them
    /// — `a_repository_whose_refs_were_not_read_has_no_track_to_draw_from` — and
    /// `app::collect::collect_repos` makes one `RepoInput` per repository, so there is no
    /// readable twin of this one to draw from either —
    /// `one_repository_is_asked_about_once_however_many_panes_are_in_it` and
    /// `a_placement_carries_one_spelling_of_a_repository_key_whichever_answered`. A fixture
    /// can still pair the two directly, and `domain::sweep::judge` offers the row on `gone`
    /// either way.
    Unreadable(String),
}

impl Refs {
    /// Whether git answered.
    ///
    /// Spelled out rather than written as `matches!(_, Unreadable(_))`, for the reason
    /// [`WorkingTree::is_drawn`] is: a third variant — refs not read yet, say — would have
    /// gone on compiling as "read" in `domain::sweep::judge`, which is the one predicate
    /// that decides whether a row says `refs unreadable`, and that is the direction it must
    /// not fail in. The compiler asks now.
    pub fn is_read(&self) -> bool {
        match self {
            Refs::Read => true,
            Refs::Unreadable(_) => false,
        }
    }
}

/// What is checked out at a path, as far as anybody has said.
///
/// Three answers rather than two, because a row with no branch name on it is two different
/// facts and nothing downstream can work out which: herdr listing the checkout and saying
/// nothing is out is an answer about the checkout, and herdr never listing it is the
/// question never having been put. `refs/heads/x` can be out at a path herdr did not list;
/// it cannot be out at one herdr listed as detached.
///
/// [`domain::tree::build`](crate::domain::tree::build) writes each of the three at a
/// construction site of its own, and
/// `what_a_branchless_row_draws_turns_on_whether_herdr_listed_it` is the two that name no
/// branch, over one repository's identical git facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Branch {
    /// herdr named the branch this checkout has out.
    Out(String),
    /// herdr listed the checkout and said nothing is out — an empty branch, or
    /// `is_detached`. No ref of the repository is about this row, so a marker on it would
    /// be about a branch the row does not name.
    NothingOut,
    /// Nobody said. `build` makes a row like this for a pane whose directory resolved to a
    /// checkout herdr's worktree list did not mention — a `git worktree add` outside herdr
    /// is the case its own comment names — so a branch may well be out and nothing on this
    /// side has heard.
    NotSaid,
}

impl Branch {
    /// The name, where a branch was named. `None` for both of the other two, which is the
    /// question [`Branch`] exists to keep answerable past this point.
    pub fn name(&self) -> Option<&str> {
        match self {
            Branch::Out(name) => Some(name),
            Branch::NothingOut | Branch::NotSaid => None,
        }
    }
}

/// Where a checkout's branch stands against what it is measured against, as far as the
/// repository's refs could say.
///
/// A bare [`Track`] and an absence were two answers for three facts. The third is
/// [`domain::tree::tracks`](crate::domain::tree) declining: more than one of the
/// repository's refs names one checkout's path, they disagree about which branch is there,
/// and a marker picked by list order is not an answer. Collapsed into the absence, that
/// refusal was a thing the domain worked out and threw away — indistinguishable from a
/// branch level with its upstream, so no reader could say it (issue #47).
///
/// [`Refs`] is the same argument one level up, about a whole repository. This is about one
/// checkout, which is the level the refusal happens at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// One ref of the repository names this checkout, and this is what git reported about
    /// it: a [`Track`], or `None` where git reported nothing — the branch is level with its
    /// upstream, or has nothing to be level with. Which of those two is the upstream's to
    /// say, and no row has both.
    Said(Option<Track>),
    /// More than one of the repository's refs names this checkout's path, so nothing here
    /// is about one branch. The domain will not choose, and nothing draws a marker for it:
    /// on the row it is the same blank as `Said(None)`, which is what issue #47 is about.
    ///
    /// The word for it lives where there is room for one.
    /// [`app::dump`](crate::app::dump) says `track contested`, and in a sweep the row says
    /// `refs disagree` — [`domain::sweep::Half`](crate::domain::sweep::Half) — because
    /// `gone` was never on offer for a checkout nobody would name a branch for. Outside a
    /// sweep the list stays silent, which is what it does for every other thing it has no
    /// marker for.
    ///
    /// `git worktree add` and `git worktree move` both refuse a path a worktree is already
    /// registered at, but `git worktree repair` will register a second and `git worktree
    /// prune` calls the result a `duplicate entry`:
    /// `one_repository_can_name_one_path_from_two_refs`.
    Contested,
    /// No ref of the repository names this checkout's path — git would not read the refs at
    /// all ([`Refs`] says so), git lists no ref there, or this is a row no ref of the
    /// repository is about, which a checkout with nothing out is.
    NotSaid,
}

/// One checkout of a repository: the primary one, or a linked worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeNode {
    /// What is out here, and — where nothing is named — which of the two reasons that is.
    /// Issue #49 is what the difference costs on the marker.
    pub branch: Branch,
    pub checkout_path: String,
    pub is_primary: bool,
    /// The workspace herdr has this checkout open in, when it has one.
    pub open_workspace_id: Option<String>,
    /// Where this checkout's branch stands against its upstream, as far as the
    /// repository's refs could say. It rides on a ref walk that is happening anyway rather
    /// than costing a process of its own; see [`port::Track`](crate::port::Track).
    pub position: Position,
    /// Panes currently working in this checkout, in the order herdr reported them.
    pub panes: Vec<PaneNode>,
}

impl WorktreeNode {
    /// A checkout with no pane in it: something the user can open, not jump to.
    pub fn is_idle(&self) -> bool {
        self.panes.is_empty()
    }

    /// What to show for the checkout. Falls back to the directory name where no branch is
    /// named, which both of [`Branch`]'s other two answers are: the path is all either row
    /// has to be called by, whatever the reason.
    pub fn label(&self) -> &str {
        self.branch.name().unwrap_or_else(|| {
            self.checkout_path
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("(detached)")
        })
    }
}

/// A checkout's path, as the key its working-tree answer and its removal are kept under.
///
/// A newtype rather than the `String` on the node, so that a map keyed by some other string a
/// node carries — [`RepoNode::repo_root`], say — cannot be passed where one keyed by this is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckoutPath(String);

impl CheckoutPath {
    pub fn of(worktree: &WorktreeNode) -> Self {
        CheckoutPath(worktree.checkout_path.clone())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub fn for_test(path: &str) -> Self {
        CheckoutPath(path.to_string())
    }
}

/// Which repository a checkout belongs to, as the other half of a key beside [`CheckoutPath`].
///
/// A newtype rather than the `String` on the node, so that half cannot be left out of a
/// key or filled from `repo_root` — the other string `RepoNode` carries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RepoKey(String);

impl RepoKey {
    pub fn of(repo: &RepoNode) -> Self {
        RepoKey(repo.repo_key.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneNode {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    /// Agent name, or a trimmed terminal title, or nothing.
    pub display_name: Option<String>,
    pub agent_status: AgentStatus,
    pub focused: bool,
}

/// Everything the panes view shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    pub repos: Vec<RepoNode>,
    /// Panes that are not inside any git work tree. Hidden by default.
    pub ungrouped: Vec<PaneNode>,
}

impl Tree {
    /// Locate the repository and checkout a pane sits in.
    pub fn find_pane(&self, pane_id: &str) -> Option<(&RepoNode, &WorktreeNode, &PaneNode)> {
        self.repos.iter().find_map(|repo| {
            repo.worktrees.iter().find_map(|worktree| {
                worktree
                    .panes
                    .iter()
                    .find(|pane| pane.pane_id == pane_id)
                    .map(|pane| (repo, worktree, pane))
            })
        })
    }

    /// Locate the repository and checkout a key names, and nothing when the tree no longer
    /// has it — a checkout marked in a sweep can be gone by the time `Enter`'s re-read is
    /// back.
    pub fn find_checkout(
        &self,
        (repo_key, path): &(RepoKey, CheckoutPath),
    ) -> Option<(&RepoNode, &WorktreeNode)> {
        let repo = self
            .repos
            .iter()
            .find(|repo| RepoKey::of(repo) == *repo_key)?;
        let worktree = repo
            .worktrees
            .iter()
            .find(|worktree| CheckoutPath::of(worktree) == *path)?;
        Some((repo, worktree))
    }
}

/// Strip trailing slashes so paths from different herdr fields compare equal. herdr returns
/// `repo_root` with a trailing slash but `worktree.path` without one.
pub fn normalize_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    // A bare "/" must not normalise to the empty string.
    if trimmed.is_empty() && path.starts_with('/') {
        "/"
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_the_trailing_slash_herdr_puts_on_repo_root() {
        assert_eq!(normalize_path("/a/b/"), "/a/b");
        assert_eq!(normalize_path("/a/b"), "/a/b");
        assert_eq!(normalize_path("/a/b///"), "/a/b");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path(""), "");
    }

    #[test]
    fn falls_back_to_the_directory_name_for_a_detached_checkout() {
        let detached = WorktreeNode {
            branch: Branch::NothingOut,
            checkout_path: "/tmp/wt/detached-head".into(),
            is_primary: false,
            open_workspace_id: None,
            position: Position::NotSaid,
            panes: vec![],
        };
        assert_eq!(detached.label(), "detached-head");

        let on_branch = WorktreeNode {
            branch: Branch::Out("feat/login".into()),
            ..detached
        };
        assert_eq!(on_branch.label(), "feat/login");
    }
}
