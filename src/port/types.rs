//! Wire types for the herdr socket API.
//!
//! These mirror the shapes in `herdr api schema` but are deliberately permissive: every
//! optional field is `#[serde(default)]` and unknown fields are ignored, so a newer herdr
//! that adds fields keeps parsing here.

use serde::Deserialize;

/// Agent lifecycle state herdr reports for a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    /// Also the catch-all for a status a newer herdr introduces.
    #[default]
    #[serde(other)]
    Unknown,
}

impl AgentStatus {
    /// Whether herdr is tracking an agent in this pane at all.
    pub fn is_tracked(self) -> bool {
        !matches!(self, AgentStatus::Unknown)
    }
}

/// One `herdr api snapshot` payload: the whole session in a single response.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Snapshot {
    pub version: String,
    pub protocol: u32,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    #[serde(default)]
    pub focused_tab_id: Option<String>,
    #[serde(default)]
    pub focused_workspace_id: Option<String>,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub panes: Vec<Pane>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub number: u32,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub active_tab_id: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    /// Present when herdr itself created this workspace for a git worktree. When it is here
    /// we can identify the repo without running git at all.
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktree>,
}

/// herdr's own record of which repo and checkout a workspace belongs to.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceWorktree {
    /// herdr's repo identity. Equivalent to the normalised `git rev-parse --git-common-dir`.
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub checkout_path: String,
    #[serde(default)]
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tab {
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub number: u32,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub pane_count: u32,
    #[serde(default)]
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub terminal_id: String,
    /// The shell's working directory. `None` for panes herdr cannot inspect.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The foreground process's working directory, which is the more accurate answer when a
    /// long-running process has chdir'd away from where the shell started.
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

impl Pane {
    /// The directory this pane is really working in. Prefers the foreground process's cwd,
    /// which follows the user into subdirectories, and falls back to the shell's.
    pub fn effective_cwd(&self) -> Option<&str> {
        self.foreground_cwd
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(self.cwd.as_deref().filter(|s| !s.is_empty()))
    }

    /// A short human label for the pane: the agent name, else a trimmed terminal title.
    pub fn display_name(&self) -> Option<&str> {
        self.agent
            .as_deref()
            .or(self.label.as_deref())
            .or(self.terminal_title_stripped.as_deref())
            .or(self.title.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

/// Result of `worktree.list`, scoped to one repository.
#[derive(Debug, Clone, Deserialize)]
pub struct WorktreeList {
    pub source: WorktreeSource,
    #[serde(default)]
    pub worktrees: Vec<Worktree>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorktreeSource {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    #[serde(default)]
    pub source_checkout_path: String,
    #[serde(default)]
    pub source_workspace_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Worktree {
    /// `None` for a detached checkout.
    #[serde(default)]
    pub branch: Option<String>,
    pub path: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub is_bare: bool,
    #[serde(default)]
    pub is_detached: bool,
    #[serde(default)]
    pub is_linked_worktree: bool,
    #[serde(default)]
    pub is_prunable: bool,
    /// The workspace this checkout is currently open in, when it is open at all.
    #[serde(default)]
    pub open_workspace_id: Option<String>,
}

/// Result of `worktree.create` / `worktree.open`. herdr always materialises a whole
/// workspace, so the caller gets a tab and root pane whether it wanted them or not.
#[derive(Debug, Clone, Deserialize)]
pub struct WorktreeOpened {
    pub workspace: Workspace,
    pub tab: Tab,
    pub root_pane: Pane,
    pub worktree: Worktree,
    /// Only meaningful for `worktree.open`.
    #[serde(default)]
    pub already_open: bool,
}
