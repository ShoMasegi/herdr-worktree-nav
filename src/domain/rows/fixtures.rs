//! The tree these tests are all written against.
//!
//! Here rather than in one submodule's `mod tests` because the same tree is what the shape
//! of the list, the cursor's path through it and the words drawn on it are each asked about
//! — the last of those from [`ui::words`](crate::ui::words), which is where the wording
//! went.

use crate::domain::model::{Branch, PaneNode, Refs, RepoNode, Tree, WorktreeNode};
use crate::domain::rows::Row;
use crate::port::AgentStatus;

pub(crate) fn pane(id: &str, name: Option<&str>, status: AgentStatus) -> PaneNode {
    let workspace = id.split(':').next().unwrap().to_string();
    PaneNode {
        pane_id: id.to_string(),
        tab_id: format!("{workspace}:t1"),
        workspace_id: workspace,
        display_name: name.map(str::to_string),
        agent_status: status,
        focused: false,
    }
}

pub(crate) fn worktree(branch: &str, panes: Vec<PaneNode>) -> WorktreeNode {
    WorktreeNode {
        branch: Branch::Out(branch.to_string()),
        checkout_path: format!("/wt/{}", branch.replace('/', "-")),
        is_primary: branch == "main",
        open_workspace_id: panes.first().map(|p| p.workspace_id.clone()),
        track: None,
        panes,
    }
}

/// `me/app` on main (a working agent and a plain shell) and feat/login (idle), plus an
/// unused fix/crash checkout; `me/site` on develop with a blocked agent.
pub(crate) fn tree() -> Tree {
    Tree {
        repos: vec![
            RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                refs: Refs::Read,
                worktrees: vec![
                    worktree(
                        "main",
                        vec![
                            pane("w1:p1", Some("claude"), AgentStatus::Working),
                            pane("w1:p2", None, AgentStatus::Unknown),
                        ],
                    ),
                    worktree(
                        "feat/login",
                        vec![pane("w2:p1", Some("codex"), AgentStatus::Idle)],
                    ),
                    worktree("fix/crash", vec![]),
                ],
            },
            RepoNode {
                repo_key: "/src/site/.git".into(),
                repo_root: "/src/site".into(),
                display_name: "me/site".into(),
                refs: Refs::Read,
                worktrees: vec![worktree(
                    "develop",
                    vec![pane("w3:p1", Some("claude"), AgentStatus::Blocked)],
                )],
            },
        ],
        ungrouped: vec![pane("w9:p1", None, AgentStatus::Unknown)],
    }
}

/// The rows as `<indent><name>`, which is the shape the tree glyphs are drawn from.
///
/// A row with no name of its own reads as `-` here, and that is all these tests say about
/// it: what an unnamed pane or the group of panes no repository holds is called is
/// [`ui::words::row_label`](crate::ui::words::row_label)'s, and asked for there.
pub(crate) fn names(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|r| {
            format!(
                "{}{}",
                "  ".repeat(r.depth as usize),
                r.name.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

pub(crate) fn find<'a>(rows: &'a [Row], name: &str) -> &'a Row {
    rows.iter()
        .find(|r| r.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no row named {name}"))
}
