//! Turning a herdr snapshot into the `repo -> worktree -> pane` tree the panes view shows.
//!
//! This is pure: the caller resolves where each pane lives and which worktrees each
//! repository has (via the ports), and hands the answers in.

use std::collections::HashMap;

use crate::domain::model::{normalize_path, PaneNode, RepoNode, Tree, WorktreeNode};
use crate::port::{Snapshot, Worktree};

/// A repository the caller has identified, together with the worktrees herdr reported for it.
#[derive(Debug, Clone)]
pub struct RepoInput {
    pub repo_key: String,
    pub repo_root: String,
    /// `owner/repo` for a GitHub origin, otherwise the directory name.
    pub display_name: String,
    pub worktrees: Vec<Worktree>,
}

/// Which repository and checkout a pane's working directory resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanePlacement {
    pub repo_key: String,
    pub checkout_path: String,
}

/// Build the tree. Panes with no placement are collected into `ungrouped`.
///
/// `exclude_pane` is the picker's own pane. It is a real pane in a real workspace, so it
/// would otherwise appear in its own list as an unnamed row the user cannot usefully jump to.
pub fn build(
    snapshot: &Snapshot,
    repos: &[RepoInput],
    placements: &HashMap<String, PanePlacement>,
    exclude_pane: Option<&str>,
) -> Tree {
    let mut nodes: Vec<RepoNode> = repos
        .iter()
        .map(|repo| RepoNode {
            repo_key: normalize_path(&repo.repo_key).to_string(),
            repo_root: normalize_path(&repo.repo_root).to_string(),
            display_name: repo.display_name.clone(),
            worktrees: repo
                .worktrees
                .iter()
                // A bare repository has no working tree, so no pane can ever sit in it.
                .filter(|worktree| !worktree.is_bare)
                .map(|worktree| WorktreeNode {
                    branch: worktree.branch.clone().filter(|b| !b.is_empty()),
                    checkout_path: normalize_path(&worktree.path).to_string(),
                    is_primary: !worktree.is_linked_worktree,
                    open_workspace_id: worktree.open_workspace_id.clone(),
                    panes: Vec::new(),
                })
                .collect(),
        })
        .collect();

    let mut by_key: HashMap<&str, usize> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        by_key.insert(node.repo_key.as_str(), index);
    }
    let by_key: HashMap<String, usize> = by_key
        .into_iter()
        .map(|(key, index)| (key.to_string(), index))
        .collect();

    let mut ungrouped = Vec::new();

    // Snapshot order is preserved within each worktree: herdr already lists panes in an
    // order that matches the layout, and re-sorting by id would put p10 before p9.
    for pane in &snapshot.panes {
        if Some(pane.pane_id.as_str()) == exclude_pane {
            continue;
        }
        let node = PaneNode {
            pane_id: pane.pane_id.clone(),
            workspace_id: pane.workspace_id.clone(),
            tab_id: pane.tab_id.clone(),
            display_name: pane.display_name().map(str::to_string),
            agent_status: pane.agent_status,
            focused: pane.focused,
        };

        let Some(placement) = placements.get(&pane.pane_id) else {
            ungrouped.push(node);
            continue;
        };
        let Some(&index) = by_key.get(normalize_path(&placement.repo_key)) else {
            ungrouped.push(node);
            continue;
        };

        let checkout = normalize_path(&placement.checkout_path);
        let repo = &mut nodes[index];
        match repo
            .worktrees
            .iter_mut()
            .find(|worktree| worktree.checkout_path == checkout)
        {
            Some(worktree) => worktree.panes.push(node),
            None => {
                // A checkout herdr's worktree list did not mention — for instance one added
                // with `git worktree add` outside herdr. Showing it is better than dropping
                // the pane into "ungrouped", where the user would not think to look.
                repo.worktrees.push(WorktreeNode {
                    branch: None,
                    checkout_path: checkout.to_string(),
                    is_primary: false,
                    open_workspace_id: Some(node.workspace_id.clone()),
                    panes: vec![node],
                });
            }
        }
    }

    for repo in &mut nodes {
        // The main checkout first, then linked worktrees alphabetically. This keeps the
        // repository's "home" row in a stable place as worktrees come and go.
        repo.worktrees.sort_by(|a, b| {
            b.is_primary
                .cmp(&a.is_primary)
                .then_with(|| a.label().cmp(b.label()))
        });
    }
    nodes.sort_by(|a, b| a.display_name.cmp(&b.display_name));

    Tree {
        repos: nodes,
        ungrouped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::AgentStatus;
    use serde_json::json;

    /// Build a snapshot from the wire shape herdr actually sends, so these tests also
    /// exercise the deserializers.
    fn snapshot(panes: serde_json::Value) -> Snapshot {
        serde_json::from_value(json!({
            "version": "0.7.4",
            "protocol": 16,
            "workspaces": [],
            "tabs": [],
            "panes": panes,
        }))
        .expect("snapshot fixture should deserialize")
    }

    fn pane(pane_id: &str, agent: Option<&str>) -> serde_json::Value {
        let (workspace_id, _) = pane_id.split_once(':').unwrap();
        json!({
            "pane_id": pane_id,
            "tab_id": format!("{workspace_id}:t1"),
            "workspace_id": workspace_id,
            "terminal_id": format!("term_{pane_id}"),
            "focused": false,
            "agent": agent,
            "agent_status": if agent.is_some() { "idle" } else { "unknown" },
        })
    }

    fn worktree(branch: &str, path: &str, linked: bool) -> Worktree {
        serde_json::from_value(json!({
            "branch": branch,
            "path": path,
            "label": branch,
            "is_bare": false,
            "is_detached": false,
            "is_linked_worktree": linked,
            "is_prunable": false,
        }))
        .expect("worktree fixture should deserialize")
    }

    fn repo(display_name: &str, root: &str, worktrees: Vec<Worktree>) -> RepoInput {
        RepoInput {
            // Matches herdr: repo_key has no trailing slash, repo_root may have one.
            repo_key: format!("{}/.git", normalize_path(root)),
            repo_root: root.to_string(),
            display_name: display_name.to_string(),
            worktrees,
        }
    }

    /// The picker's own pane is a runtime concern; these tests are about grouping.
    fn build_for_test(
        snapshot: &Snapshot,
        repos: &[RepoInput],
        placements: &HashMap<String, PanePlacement>,
    ) -> Tree {
        build(snapshot, repos, placements, None)
    }

    fn placements(pairs: &[(&str, &str, &str)]) -> HashMap<String, PanePlacement> {
        pairs
            .iter()
            .map(|(pane_id, repo_key, checkout)| {
                (
                    (*pane_id).to_string(),
                    PanePlacement {
                        repo_key: (*repo_key).to_string(),
                        checkout_path: (*checkout).to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn groups_panes_under_the_worktree_they_sit_in() {
        let tree = build_for_test(
            &snapshot(json!([pane("w1:p1", Some("claude")), pane("w2:p1", None)])),
            &[repo(
                "me/app",
                "/src/app",
                vec![
                    worktree("main", "/src/app", false),
                    worktree("feat/login", "/wt/app/feat-login", true),
                ],
            )],
            &placements(&[
                ("w1:p1", "/src/app/.git", "/src/app"),
                ("w2:p1", "/src/app/.git", "/wt/app/feat-login"),
            ]),
        );

        assert_eq!(tree.repos.len(), 1);
        let repo = &tree.repos[0];
        assert_eq!(repo.display_name, "me/app");
        assert_eq!(repo.worktrees.len(), 2);
        assert_eq!(repo.worktrees[0].label(), "main");
        assert_eq!(repo.worktrees[0].panes[0].pane_id, "w1:p1");
        assert_eq!(
            repo.worktrees[0].panes[0].display_name.as_deref(),
            Some("claude")
        );
        assert_eq!(repo.worktrees[1].label(), "feat/login");
        assert_eq!(repo.worktrees[1].panes[0].pane_id, "w2:p1");
        assert!(tree.ungrouped.is_empty());
    }

    #[test]
    fn keeps_worktrees_that_have_no_pane_as_openable_rows() {
        let tree = build_for_test(
            &snapshot(json!([pane("w1:p1", Some("claude"))])),
            &[repo(
                "me/app",
                "/src/app",
                vec![
                    worktree("main", "/src/app", false),
                    worktree("fix/crash", "/wt/app/fix-crash", true),
                ],
            )],
            &placements(&[("w1:p1", "/src/app/.git", "/src/app")]),
        );

        let idle = &tree.repos[0].worktrees[1];
        assert_eq!(idle.label(), "fix/crash");
        assert!(
            idle.is_idle(),
            "a checkout with no pane should read as idle"
        );
    }

    #[test]
    fn puts_panes_outside_any_repository_into_ungrouped() {
        let tree = build_for_test(
            &snapshot(json!([pane("w1:p1", None), pane("w9:p1", None)])),
            &[repo(
                "me/app",
                "/src/app",
                vec![worktree("main", "/src/app", false)],
            )],
            &placements(&[("w1:p1", "/src/app/.git", "/src/app")]),
        );

        assert_eq!(tree.ungrouped.len(), 1);
        assert_eq!(tree.ungrouped[0].pane_id, "w9:p1");
    }

    #[test]
    fn matches_a_pane_to_its_worktree_despite_herdrs_trailing_slashes() {
        let tree = build_for_test(
            &snapshot(json!([pane("w1:p1", None)])),
            &[repo(
                "me/app",
                "/src/app/",
                vec![worktree("main", "/src/app", false)],
            )],
            // herdr reports repo_root with a trailing slash and worktree.path without one.
            &placements(&[("w1:p1", "/src/app/.git", "/src/app/")]),
        );

        assert_eq!(tree.repos[0].worktrees[0].panes.len(), 1);
        assert!(tree.ungrouped.is_empty());
    }

    #[test]
    fn synthesizes_a_row_for_a_checkout_herdr_did_not_list() {
        let tree = build_for_test(
            &snapshot(json!([pane("w4:p1", Some("codex"))])),
            &[repo(
                "me/app",
                "/src/app",
                vec![worktree("main", "/src/app", false)],
            )],
            &placements(&[("w4:p1", "/src/app/.git", "/elsewhere/manual-worktree")]),
        );

        assert!(tree.ungrouped.is_empty(), "the pane must not be lost");
        let synthesized = tree.repos[0]
            .worktrees
            .iter()
            .find(|w| w.checkout_path == "/elsewhere/manual-worktree")
            .expect("the unknown checkout should appear under its repository");
        assert_eq!(synthesized.panes[0].pane_id, "w4:p1");
        assert_eq!(synthesized.label(), "manual-worktree");
    }

    #[test]
    fn orders_repositories_alphabetically_and_the_main_checkout_first() {
        let tree = build_for_test(
            &snapshot(json!([])),
            &[
                repo(
                    "me/zeta",
                    "/src/zeta",
                    vec![worktree("main", "/src/zeta", false)],
                ),
                repo(
                    "me/alpha",
                    "/src/alpha",
                    vec![
                        worktree("feat/b", "/wt/alpha/b", true),
                        worktree("feat/a", "/wt/alpha/a", true),
                        worktree("main", "/src/alpha", false),
                    ],
                ),
            ],
            &placements(&[]),
        );

        let names: Vec<_> = tree.repos.iter().map(|r| r.display_name.as_str()).collect();
        assert_eq!(names, ["me/alpha", "me/zeta"]);
        let labels: Vec<_> = tree.repos[0].worktrees.iter().map(|w| w.label()).collect();
        assert_eq!(labels, ["main", "feat/a", "feat/b"]);
    }

    #[test]
    fn preserves_the_snapshot_order_of_panes_within_a_worktree() {
        // Sorting by pane id would put p10 before p9; herdr's order is the layout order.
        let tree = build_for_test(
            &snapshot(json!([
                pane("w1:p9", None),
                pane("w1:p10", None),
                pane("w1:p1", None)
            ])),
            &[repo(
                "me/app",
                "/src/app",
                vec![worktree("main", "/src/app", false)],
            )],
            &placements(&[
                ("w1:p9", "/src/app/.git", "/src/app"),
                ("w1:p10", "/src/app/.git", "/src/app"),
                ("w1:p1", "/src/app/.git", "/src/app"),
            ]),
        );

        let ids: Vec<_> = tree.repos[0].worktrees[0]
            .panes
            .iter()
            .map(|p| p.pane_id.as_str())
            .collect();
        assert_eq!(ids, ["w1:p9", "w1:p10", "w1:p1"]);
    }

    #[test]
    fn skips_bare_repositories_which_can_never_hold_a_pane() {
        let mut bare = worktree("main", "/src/app.git", false);
        bare.is_bare = true;
        let tree = build_for_test(
            &snapshot(json!([])),
            &[repo("me/app", "/src/app", vec![bare])],
            &placements(&[]),
        );
        assert!(tree.repos[0].worktrees.is_empty());
    }

    #[test]
    fn leaves_the_pickers_own_pane_out_of_the_list_it_draws() {
        // The overlay is a real pane in a real workspace, so without this it appears in its
        // own list as a row the user cannot usefully jump to.
        let tree = build(
            &snapshot(json!([pane("w1:p1", Some("claude")), pane("w1:p5", None)])),
            &[repo(
                "me/app",
                "/src/app",
                vec![worktree("main", "/src/app", false)],
            )],
            &placements(&[
                ("w1:p1", "/src/app/.git", "/src/app"),
                ("w1:p5", "/src/app/.git", "/src/app"),
            ]),
            Some("w1:p5"),
        );
        let ids: Vec<_> = tree.repos[0].worktrees[0]
            .panes
            .iter()
            .map(|p| p.pane_id.as_str())
            .collect();
        assert_eq!(ids, ["w1:p1"]);
        assert!(tree.ungrouped.is_empty());
    }

    #[test]
    fn finds_which_repository_and_checkout_a_pane_belongs_to() {
        let tree = build_for_test(
            &snapshot(json!([pane("w2:p1", Some("claude"))])),
            &[repo(
                "me/app",
                "/src/app",
                vec![
                    worktree("main", "/src/app", false),
                    worktree("feat/login", "/wt/app/feat-login", true),
                ],
            )],
            &placements(&[("w2:p1", "/src/app/.git", "/wt/app/feat-login")]),
        );

        let (repo, worktree, pane) = tree.find_pane("w2:p1").expect("pane should be found");
        assert_eq!(repo.display_name, "me/app");
        assert_eq!(worktree.label(), "feat/login");
        assert_eq!(pane.agent_status, AgentStatus::Idle);
        assert!(tree.find_pane("w9:p9").is_none());
    }
}
