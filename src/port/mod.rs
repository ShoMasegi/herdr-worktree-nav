//! The boundary between this plugin and the outside world.
//!
//! Everything above this module works against these traits, never against a process or a
//! socket, so the interesting logic in `crate::domain` can be tested without a running
//! herdr server or a real repository.

pub mod types;

use std::num::NonZeroU32;

use anyhow::Result;
pub use types::*;

/// Where a worktree pane should end up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneDestination {
    /// Split an existing tab. `target_pane_id` picks which pane inside it is split.
    Tab {
        tab_id: String,
        split: SplitDirection,
        target_pane_id: Option<String>,
    },
    /// Add a new tab, to the given workspace or the pane's current one.
    NewTab { workspace_id: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Right,
    Down,
}

impl SplitDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            SplitDirection::Right => "right",
            SplitDirection::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct WorktreeCreate {
    /// Any path inside the repository the worktree is cut from.
    pub cwd: String,
    pub branch: Option<String>,
    /// Base ref for a branch that does not exist yet.
    pub base: Option<String>,
    pub focus: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorktreeOpen {
    pub cwd: String,
    pub path: Option<String>,
    pub branch: Option<String>,
    pub focus: bool,
}

#[derive(Debug, Clone)]
pub struct PaneSplit {
    pub target_pane_id: String,
    pub direction: SplitDirection,
    pub cwd: Option<String>,
    pub focus: bool,
}

#[derive(Debug, Clone)]
pub struct PluginPaneOpen {
    pub plugin_id: String,
    pub entrypoint: String,
    pub cwd: Option<String>,
    /// Extra environment for the pane process. Used to tell the picker which pane summoned
    /// it, which its own environment cannot say.
    pub env: Vec<(String, String)>,
    pub focus: bool,
}

/// Why a `plugin.pane.open` did not open anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRefusal {
    /// herdr allows one popup at a time and one is already up.
    PopupAlreadyOpen,
}

/// Everything this plugin asks of herdr.
///
/// `Sync` because opening a branch runs on a background thread while the picker keeps
/// drawing: a fetch and a checkout are seconds of work, and a picker that freezes for them
/// looks broken.
pub trait HerdrPort: Sync {
    /// The whole session in one call: workspaces, tabs, panes.
    fn snapshot(&self) -> Result<Snapshot>;

    /// Every worktree of the repository containing `cwd`, including ones no workspace has
    /// open (those carry `open_workspace_id: None`).
    fn worktree_list(&self, cwd: &str) -> Result<WorktreeList>;

    /// Create a checkout in herdr's configured worktree directory. Note that herdr always
    /// materialises a new workspace for it; see `docs/adr/0001-delegate-worktree-creation.md`.
    fn worktree_create(&self, req: &WorktreeCreate) -> Result<WorktreeOpened>;

    /// Open an existing checkout as a workspace.
    fn worktree_open(&self, req: &WorktreeOpen) -> Result<WorktreeOpened>;

    /// Focus one specific pane. Only reachable over the socket — the CLI's `pane focus` is
    /// directional and `agent focus` rejects panes with no agent.
    fn pane_focus(&self, pane_id: &str) -> Result<()>;

    fn pane_split(&self, req: &PaneSplit) -> Result<Pane>;

    /// Make sure this pane is not running, stopping whatever was.
    ///
    /// A pane that has already gone is not an error: it is the state this asks for, and it
    /// arrives on its own whenever a pane's command finishes.
    ///
    /// The first call here that takes something out of the session rather than adding to it
    /// or rearranging it — see `docs/adr/0010-closing-the-panes-first.md`. herdr collapses a
    /// tab and a workspace that end up empty, which is what lets this leave no residue:
    /// measured against herdr 0.7.4 by closing the only pane of a fresh workspace, which
    /// took its tab and its workspace with it and left every other one alone.
    fn pane_close(&self, pane_id: &str) -> Result<()>;

    /// Relocate a pane. herdr closes the tab and workspace the pane leaves behind if they
    /// end up empty, which is what makes the "create then move" flow leave no residue.
    fn pane_move(&self, pane_id: &str, dest: &PaneDestination, focus: bool) -> Result<()>;

    fn workspace_focus(&self, workspace_id: &str) -> Result<()>;
    fn tab_focus(&self, tab_id: &str) -> Result<()>;

    /// Open one of the manifest's pane entrypoints. Nothing is returned: these open as
    /// popups, and a popup is a singleton session resource with no pane id.
    ///
    /// `Ok(Some(refusal))` is herdr declining for a reason the caller can act on, as opposed
    /// to an error.
    fn plugin_pane_open(&self, req: &PluginPaneOpen) -> Result<Option<OpenRefusal>>;

    /// Show a toast. The only call here that says something to the user rather than doing
    /// something to the session, and the only thing this plugin can do once its own window
    /// has gone — which is what a removal that outlives the picker needs.
    ///
    /// herdr may decline to show it: notifications turned off, no client attached, too many
    /// already. That is an answer rather than a failure, and it is one this plugin accepts
    /// in silence — see `docs/adr/0014-removing-outlives-the-picker.md`.
    fn notify(&self, notification: &Notification) -> Result<()>;
}

/// Starting a removal that outlives this process.
///
/// `git worktree remove` walks a whole working tree before it deletes it, which is seconds
/// on a repository of any size. The picker starts one and is then free to be closed; the
/// process that carries it out reports itself. See
/// `docs/adr/0014-removing-outlives-the-picker.md`.
pub trait RemovalPort {
    /// Start removing `checkout_path`. `label` is the branch, which is what the report
    /// names — the reader was waiting on a branch, not on a path. `panes_closed` is what was
    /// stopped to get here: the caller has to say, because by the time this runs the panes
    /// are gone and there is nothing left to count.
    fn start(
        &self,
        repo_root: &str,
        checkout_path: &str,
        label: &str,
        panes_closed: usize,
    ) -> Result<Box<dyn RunningRemoval>>;
}

/// A removal already running somewhere else.
///
/// `Send`, because waiting for it is the one thing the picker cannot do on the thread it
/// draws with. Dropping it without waiting is not an abandonment: the removal carries on and
/// reports itself either way.
pub trait RunningRemoval: Send {
    /// Wait for it to finish and read what it said. `Err` when it ended without saying
    /// anything this side can read, which is the one case neither outcome covers.
    fn wait(self: Box<Self>) -> Result<RemovalOutcome>;
}

/// A branch reference as git reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRef {
    /// Short branch name with no `refs/heads/` or remote prefix, e.g. `feat/login`.
    pub name: String,
    pub kind: RefKind,
    /// Committer date, for ordering most-recent-first. `None` when unknown.
    pub committed_at: Option<i64>,
    pub subject: Option<String>,
    /// Where this branch stands against the upstream it tracks — or, for a branch with no
    /// upstream configured, against where it would push. `None` when it is level with
    /// whichever of those it was measured against, and when there is neither.
    pub track: Option<Track>,
    /// The checkout that currently has this branch, when one does. git answers this in the
    /// same breath as everything else here, which is what ties a branch to a checkout
    /// without having to assume that two things named `feat/login` are the same one.
    pub worktree_path: Option<String>,
}

/// What git says about a branch's position relative to the upstream it tracks.
///
/// Read out of `%(upstream:track)`, or `%(push:track)` for a branch with no upstream
/// configured, both of which the one `for-each-ref` this plugin already runs prints
/// alongside everything else it is being asked for. The alternative is a `rev-list --count`
/// per branch.
///
/// Four variants for the four things git prints. A branch level with what it is measured
/// against prints nothing at all, so there is no variant for it and no way to build one:
/// `NonZeroU32` is what makes "at least one side is non-zero" a fact about the type rather
/// than a sentence in a doc comment that two other modules had to defend against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Track {
    /// git could not find the ref this branch tracks. Usually that is a merged pull request
    /// whose head GitHub deleted and a pruning fetch then dropped, which is what makes it
    /// worth showing — but a never-fetched upstream and a hand-pruned `refs/remotes` read
    /// the same, because to git they are the same: the ref is not there.
    Gone,
    /// Commits on this branch that the ref it tracks does not have.
    Ahead(NonZeroU32),
    /// Commits on the ref this branch tracks that the branch does not have.
    Behind(NonZeroU32),
    /// Commits each side has that the other does not. Both counts, because which one is
    /// larger is not the question — that the two have parted at all is.
    Diverged {
        ahead: NonZeroU32,
        behind: NonZeroU32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Local,
    /// A `refs/remotes/<remote>/…` ref that has been fetched at least once.
    Remote,
}

/// Identity of a repository, as resolved from any path inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIdentity {
    /// Normalised `git rev-parse --git-common-dir`. Shared by a repo and all its worktrees,
    /// and equal to herdr's own `repo_key`.
    pub repo_key: String,
    /// Top level of the checkout the queried path is in.
    pub checkout_path: String,
    /// Current branch of that checkout, `None` when detached.
    pub branch: Option<String>,
}

/// `Sync` because pane working directories are resolved from several threads at once, and
/// `Send` because the port is shared into detached threads through an `Arc` — which requires
/// both. It is detached rather than scoped because asking whether a checkout is dirty
/// outlives the view that asked: those answers are wanted on both sides of a `Tab`.
pub trait GitPort: Send + Sync {
    /// Resolve which repository and branch a directory belongs to. `Ok(None)` when the path
    /// is not inside a work tree — that is an ordinary answer, not an error.
    fn identify(&self, cwd: &str) -> Result<Option<RepoIdentity>>;

    /// `owner/repo` when `origin` points at GitHub, otherwise `None`.
    fn github_slug(&self, repo_root: &str) -> Result<Option<String>>;

    /// Local and already-fetched remote branches. Cheap and offline.
    fn local_refs(&self, repo_root: &str) -> Result<Vec<GitRef>>;

    /// Branch names on the remote, including ones never fetched. Requires the network.
    fn remote_heads(&self, repo_root: &str) -> Result<Vec<String>>;

    /// Fetch one branch so a worktree can be cut from it.
    fn fetch_branch(&self, repo_root: &str, branch: &str) -> Result<()>;

    /// Bring every remote-tracking ref up to date, and drop the ones whose branch is gone
    /// from the remote. What a person means by "fetch the repository".
    fn fetch_all(&self, repo_root: &str) -> Result<()>;

    /// Delete a linked worktree: its checkout and git's record of it. The branch it was on
    /// is left alone, and git refuses when the checkout has uncommitted work — neither of
    /// which this plugin overrides.
    fn remove_worktree(&self, repo_root: &str, checkout_path: &str) -> Result<()>;

    /// Whether this checkout is holding work that is not committed: modified tracked files,
    /// or untracked ones. The same question `git worktree remove` asks before it refuses,
    /// which is why untracked files count.
    ///
    /// One process per checkout, and the only thing here that cannot be folded into an
    /// existing call — so it is asked in the background rather than in front of the first
    /// frame.
    fn is_dirty(&self, checkout_path: &str) -> Result<bool>;

    /// Current `HEAD` of `repo_root`, used as the base for a brand new branch.
    fn head_ref(&self, repo_root: &str) -> Result<String>;
}

/// A pull request, used only to annotate branches. Never load-bearing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub head_ref: String,
    pub is_draft: bool,
}

/// `Sync` because the pull request lookup runs on a background thread while the picker
/// is already on screen.
pub trait GhPort: Sync {
    /// Open pull requests for the repository, or an empty list when `gh` is missing or
    /// unauthenticated. This layer is decoration: it must never fail the picker.
    fn pull_requests(&self, repo_root: &str) -> Vec<PullRequest>;
}
