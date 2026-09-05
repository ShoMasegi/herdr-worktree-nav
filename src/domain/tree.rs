//! Turning a herdr snapshot into the `repo -> worktree -> pane` tree the panes view shows.
//!
//! This is pure: the caller resolves where each pane lives and which worktrees each
//! repository has (via the ports), and hands the answers in.

use std::collections::HashMap;

use crate::domain::model::{normalize_path, PaneNode, RepoNode, Tree, WorktreeNode};
use crate::port::{GitRef, RefKind, Snapshot, Track, Worktree};

/// A repository the caller has identified, together with the worktrees herdr reported for it.
#[derive(Debug, Clone)]
pub struct RepoInput {
    pub repo_key: String,
    pub repo_root: String,
    /// `owner/repo` for a GitHub origin, otherwise the directory name.
    pub display_name: String,
    pub worktrees: Vec<Worktree>,
    /// The repository's refs, for what git says about each branch's upstream. Empty when
    /// the read failed: a checkout simply carries no marker then, which is the same thing
    /// it does for a branch with nothing to report.
    pub refs: Vec<GitRef>,
}

/// What git said about the branch each checkout has out, by checkout.
///
/// Keyed on `%(worktreepath)` rather than on the branch name. git is answering "which
/// checkout has this ref", which is the question being asked here, and it is right about a
/// detached checkout — nothing points at it, so it is absent and gets no marker — where a
/// name match would have to guess.
///
/// One map over every repository rather than a scan per checkout. The lookup for a checkout
/// herdr did not list used to reach into `repos` by an index that was only valid because
/// `nodes` happened to be built from it in order, and a `filter` added to that `map` would
/// have silently attached one repository's branch state to another's checkouts.
///
/// Safe to flatten every repository into one map because the key is a working tree's
/// absolute path, and a directory is the working tree of at most one repository — so two
/// repositories cannot offer the same key, and the `collect` below has no duplicate to
/// silently drop. Both the key and the lookup go through `normalize_path`, which is what
/// makes that true of the strings rather than only of the directories.
fn tracks(repos: &[RepoInput]) -> HashMap<&str, Track> {
    repos
        .iter()
        .flat_map(|repo| &repo.refs)
        .filter(|git_ref| git_ref.kind == RefKind::Local)
        .filter_map(|git_ref| {
            Some((
                normalize_path(git_ref.worktree_path.as_deref()?),
                git_ref.track?,
            ))
        })
        .collect()
}

/// Which repository and checkout a pane's working directory resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanePlacement {
    pub repo_key: String,
    pub checkout_path: String,
}

/// Build the tree. Panes with no placement are collected into `ungrouped`.
///
/// Nothing needs excluding: the picker runs as a popup, which herdr does not report as a
/// pane, so it never appears in its own list.
pub fn build(
    snapshot: &Snapshot,
    repos: &[RepoInput],
    placements: &HashMap<String, PanePlacement>,
) -> Tree {
    let tracks = tracks(repos);
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
                    track: tracks.get(normalize_path(&worktree.path)).copied(),
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
                    // git knows about it even where herdr does not.
                    track: tracks.get(checkout).copied(),
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
    use std::num::NonZeroU32;

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
            refs: Vec::new(),
        }
    }

    fn local_ref(name: &str, worktree_path: Option<&str>, track: Option<Track>) -> GitRef {
        GitRef {
            name: name.to_string(),
            kind: RefKind::Local,
            committed_at: None,
            subject: None,
            track,
            worktree_path: worktree_path.map(str::to_string),
        }
    }

    #[test]
    fn a_checkout_carries_what_git_said_about_the_branch_it_has_out() {
        let mut input = repo(
            "me/app",
            "/src/app",
            vec![
                worktree("main", "/src/app", false),
                worktree("feat/login", "/wt/feat-login", true),
            ],
        );
        input.refs = vec![
            local_ref("main", Some("/src/app"), None),
            local_ref(
                "feat/login",
                Some("/wt/feat-login"),
                Some(Track::Diverged {
                    ahead: NonZeroU32::new(2).unwrap(),
                    behind: NonZeroU32::new(1).unwrap(),
                }),
            ),
        ];

        let tree = build(&snapshot(json!([])), &[input], &HashMap::new());
        let worktrees = &tree.repos[0].worktrees;
        assert_eq!(worktrees[0].track, None);
        assert_eq!(
            worktrees[1].track,
            Some(Track::Diverged {
                ahead: NonZeroU32::new(2).unwrap(),
                behind: NonZeroU32::new(1).unwrap()
            })
        );
    }

    #[test]
    fn a_checkout_herdr_did_not_list_still_gets_what_git_said_about_it() {
        // The path the index-based lookup used to take, and the reason it was replaced: it
        // reached into `repos` by an index that was only valid because `nodes` happened to
        // be built from it in order.
        let mut input = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );
        input.refs = vec![
            local_ref("main", Some("/src/app"), None),
            local_ref("manual", Some("/elsewhere/manual"), Some(Track::Gone)),
        ];
        let tree = build(
            &snapshot(json!([pane("w1:p1", None)])),
            &[input],
            &placements(&[("w1:p1", "/src/app/.git", "/elsewhere/manual")]),
        );
        let synthesized = tree.repos[0]
            .worktrees
            .iter()
            .find(|worktree| worktree.checkout_path == "/elsewhere/manual")
            .expect("herdr did not list it, so the pane's own cwd put it there");
        assert_eq!(synthesized.track, Some(Track::Gone));
    }

    #[test]
    fn one_repositorys_branch_state_never_lands_on_anothers_checkout() {
        // Two repositories, a branch of the same name in each, and no order between them
        // that the lookup is allowed to depend on. Checkout paths are absolute, so they are
        // what tells the two apart.
        let mut app = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );
        app.refs = vec![local_ref("main", Some("/src/app"), Some(Track::Gone))];
        let mut site = repo(
            "me/site",
            "/src/site",
            vec![worktree("main", "/src/site", false)],
        );
        site.refs = vec![local_ref("main", Some("/src/site"), None)];

        let tree = build(&snapshot(json!([])), &[app, site], &HashMap::new());
        assert_eq!(tree.repos[0].worktrees[0].track, Some(Track::Gone));
        assert_eq!(tree.repos[1].worktrees[0].track, None, "not the other's");
    }

    #[test]
    fn the_branch_is_matched_by_the_checkout_git_says_has_it() {
        // Not by name. A ref that is not checked out anywhere says nothing about a checkout
        // that merely shares its name, and a detached checkout has nothing pointing at it —
        // which is exactly right: no marker rather than the wrong one.
        let mut input = repo(
            "me/app",
            "/src/app",
            vec![
                worktree("main", "/src/app", false),
                worktree("", "/wt/detached", true),
            ],
        );
        input.refs = vec![
            local_ref("main", Some("/src/app"), Some(Track::Gone)),
            local_ref("feat/login", None, Some(Track::Gone)),
        ];

        let tree = build(&snapshot(json!([])), &[input], &HashMap::new());
        let worktrees = &tree.repos[0].worktrees;
        assert_eq!(worktrees[0].track, Some(Track::Gone));
        assert_eq!(worktrees[1].branch, None, "detached");
        assert_eq!(worktrees[1].track, None);
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
        let tree = build(
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
        let tree = build(
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
        let tree = build(
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
        let tree = build(
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
        let tree = build(
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
        let tree = build(
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
        let tree = build(
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
        let tree = build(
            &snapshot(json!([])),
            &[repo("me/app", "/src/app", vec![bare])],
            &placements(&[]),
        );
        assert!(tree.repos[0].worktrees.is_empty());
    }

    #[test]
    fn finds_which_repository_and_checkout_a_pane_belongs_to() {
        let tree = build(
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
