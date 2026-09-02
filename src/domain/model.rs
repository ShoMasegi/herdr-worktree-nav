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
}

/// One checkout of a repository: the primary one, or a linked worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeNode {
    /// `None` for a detached checkout.
    pub branch: Option<String>,
    pub checkout_path: String,
    /// The main checkout rather than a linked worktree.
    pub is_primary: bool,
    /// The workspace herdr has this checkout open in, when it has one.
    pub open_workspace_id: Option<String>,
    /// Where this checkout's branch stands against its upstream, when it has anything to
    /// say. Free with the ref walk that is already happening; see `port::Track`.
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
