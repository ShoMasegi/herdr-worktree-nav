//! The model both pickers display: repositories, their worktrees, and the panes sitting in
//! each one.

use std::collections::BTreeMap;

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
    /// git answered. A checkout here with no track marker has nothing git reported for it —
    /// or is one `domain::tree::tracks` answered with nothing, because two refs of its own
    /// repository name its path.
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

/// One checkout of a repository: the primary one, or a linked worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeNode {
    /// `None` for a checkout herdr listed with nothing out — and also for one herdr never
    /// listed, which [`domain::tree::build`](crate::domain::tree::build) synthesizes for a
    /// pane and where a branch may well be out. Neither carries a `track`, because a marker
    /// about a branch the row does not name is one nobody can act on:
    /// `a_branchless_row_draws_no_track_whether_or_not_herdr_listed_it`. Carrying the
    /// difference between the two is issue #52; until then the second loses a marker it
    /// might have deserved, which is the cheaper of the two mistakes.
    pub branch: Option<String>,
    pub checkout_path: String,
    pub is_primary: bool,
    /// The workspace herdr has this checkout open in, when it has one.
    pub open_workspace_id: Option<String>,
    /// Where this checkout's branch stands against its upstream, when it has anything to
    /// say. It rides on a ref walk that is happening anyway rather than costing a process
    /// of its own; see [`port::Track`](crate::port::Track).
    pub track: Option<Track>,
    /// Panes currently working in this checkout, in the order herdr reported them.
    pub panes: Vec<PaneNode>,
}

impl WorktreeNode {
    /// A checkout with no pane in it: something the user can open, not jump to.
    pub fn is_idle(&self) -> bool {
        self.panes.is_empty()
    }

    /// What to show for the checkout. Falls back to the directory name when detached.
    pub fn label(&self) -> &str {
        self.branch.as_deref().unwrap_or_else(|| {
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

/// What the reading could not do, kept on the tree the way [`Refs`] is kept on a repository.
///
/// [`Refs::Unreadable`] is the one place this model had for "the tool would not answer", and
/// it hangs off a [`RepoNode`] — which is exactly the node that does not exist when herdr
/// will not list the repository at all. So the fact has nowhere to go and the repository
/// simply leaves the tree, with every checkout and every pane in it going too and nothing
/// anywhere saying why. Here it travels with the tree, and the prompt line says it once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trouble {
    /// Repositories herdr refused to list the worktrees of.
    pub unlisted: Vec<Unlisted>,
    /// Panes git could not be asked about: the pane id, and git's own words. A `git` that is
    /// not on the path herdr launched the plugin with fails for every one of them at once,
    /// and the whole session then draws under `not in any repository` — which is what herdr
    /// not seeing into a pane also looks like, and the two are nothing alike to fix.
    pub unplaced: BTreeMap<String, String>,
}

/// A repository herdr would not list, in herdr's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unlisted {
    /// What herdr was asked about. The only name this side has: `repo_root` and the display
    /// name both come out of the listing that did not happen.
    pub repo_key: String,
    /// herdr's own words.
    pub words: String,
}

impl Unlisted {
    /// What to call the repository on screen. The directory its key sits in —
    /// `/src/app/.git` is `app` — since that is the name a reader would recognise, and the
    /// whole key where there is no such directory.
    pub fn name(&self) -> &str {
        let key = normalize_path(&self.repo_key);
        let root = key.strip_suffix("/.git").unwrap_or(key);
        match root.rsplit_once('/') {
            Some((_, name)) if !name.is_empty() => name,
            _ => &self.repo_key,
        }
    }
}

/// Everything the panes view shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    pub repos: Vec<RepoNode>,
    /// Panes that are not inside any git work tree. Hidden by default.
    pub ungrouped: Vec<PaneNode>,
    /// What this reading could not read at all. Not a property of anything on screen, which
    /// is the point: it is about what is missing from it.
    pub trouble: Trouble,
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
    fn a_repository_that_was_never_listed_is_named_by_the_directory_its_key_sits_in() {
        // The listing is where `me/app` and the repository root would both have come from,
        // so the key is all there is. `/src/app/.git` is `app` to a reader; the key itself
        // is not, and is only right where there is nothing better.
        let named = |key: &str| Unlisted {
            repo_key: key.to_string(),
            words: String::new(),
        };
        assert_eq!(named("/src/app/.git").name(), "app");
        assert_eq!(named("/src/app/.git/").name(), "app");
        // A bare repository, and a worktree's own `.git` file resolved to a common dir that
        // is not called `.git`: the last segment is still the name a reader recognises.
        assert_eq!(named("/src/app.git").name(), "app.git");
        assert_eq!(named("/src/app").name(), "app");
        // Nothing better to say than what herdr was asked about.
        assert_eq!(named(".git").name(), ".git");
        assert_eq!(named("").name(), "");
    }

    #[test]
    fn falls_back_to_the_directory_name_for_a_detached_checkout() {
        let detached = WorktreeNode {
            branch: None,
            checkout_path: "/tmp/wt/detached-head".into(),
            is_primary: false,
            open_workspace_id: None,
            track: None,
            panes: vec![],
        };
        assert_eq!(detached.label(), "detached-head");

        let on_branch = WorktreeNode {
            branch: Some("feat/login".into()),
            ..detached
        };
        assert_eq!(on_branch.label(), "feat/login");
    }
}
