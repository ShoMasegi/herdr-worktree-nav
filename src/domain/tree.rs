//! Turning a herdr snapshot into the `repo -> worktree -> pane` tree the panes view shows.
//!
//! The caller resolves where each pane lives and which worktrees each repository has, via
//! the ports, and hands the answers in.

use std::collections::HashMap;

use crate::domain::model::{normalize_path, PaneNode, Refs, RepoNode, Tree, Trouble, WorktreeNode};
use crate::port::{GitRef, RefKind, Snapshot, Track, Worktree};

/// A repository the caller has identified, together with the worktrees herdr reported for it.
#[derive(Debug, Clone)]
pub struct RepoInput {
    pub repo_key: String,
    pub repo_root: String,
    /// `owner/repo` for a GitHub origin, otherwise the directory name.
    pub display_name: String,
    pub worktrees: Vec<Worktree>,
    /// The repository's refs, for what git says about each branch's upstream — or git's own
    /// words when the read failed. A checkout carries no marker then, which is the same
    /// thing it does for a branch with nothing to report, and the repository says so
    /// instead: see [`Refs`].
    pub refs: Result<Vec<GitRef>, String>,
}

/// What git said about the branch each checkout has out, by repository and checkout.
///
/// Keyed on `%(worktreepath)` rather than on the branch name: git is answering "which
/// checkout has this ref", which is the question being asked here, where a name match would
/// have to guess.
///
/// The repository is half the key. A directory is the working tree of at most one
/// repository, but git goes on listing a worktree whose directory was moved away, so two
/// repositories can name one path —
/// `two_repositories_can_name_one_path_and_prune_will_not_part_them` in `tests/git_adapter.rs`.
/// Keyed on the path alone, whichever came last won, and a stale `[gone]` reached a live
/// checkout in another repository: issue #31.
///
/// A repository that names one of its own paths from two refs is answered with nothing
/// rather than with either: they disagree about which branch is at that path, and a marker
/// picked by list order is not an answer. `one_repository_can_name_one_path_from_two_refs`
/// is that state.
fn tracks(repos: &[RepoInput]) -> HashMap<(&str, &str), Track> {
    let mut found: HashMap<(&str, &str), Option<Track>> = HashMap::new();
    for repo in repos {
        let repo_key = normalize_path(&repo.repo_key);
        for git_ref in repo.refs.as_deref().unwrap_or_default() {
            if git_ref.kind != RefKind::Local {
                continue;
            }
            let Some(path) = git_ref.worktree_path.as_deref() else {
                continue;
            };
            found
                .entry((repo_key, normalize_path(path)))
                .and_modify(|held| *held = None)
                .or_insert(git_ref.track);
        }
    }
    found
        .into_iter()
        .filter_map(|(key, track)| Some((key, track?)))
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
            refs: match &repo.refs {
                Ok(_) => Refs::Read,
                Err(words) => Refs::Unreadable(words.clone()),
            },
            worktrees: repo
                .worktrees
                .iter()
                // A bare repository has no working tree, so no pane can ever sit in it.
                .filter(|worktree| !worktree.is_bare)
                .map(|worktree| {
                    let checkout_path = normalize_path(&worktree.path);
                    let branch = worktree
                        .branch
                        .clone()
                        .filter(|b| !b.is_empty())
                        .filter(|_| !worktree.is_detached);
                    // herdr says nothing is checked out here — `branch` absent or empty, or
                    // `is_detached` — so no ref of this repository's is about it. git can
                    // still name the path from a registration that lost its directory,
                    // carrying `[gone]`:
                    // `a_ref_carrying_gone_can_name_a_path_whose_checkout_has_no_branch_out`
                    // in `tests/git_adapter.rs`. The row `build` makes below for a pane
                    // herdr never listed reads the same way, for the same reason.
                    let track = if branch.is_some() {
                        tracks
                            .get(&(normalize_path(&repo.repo_key), checkout_path))
                            .copied()
                    } else {
                        None
                    };
                    WorktreeNode {
                        branch,
                        checkout_path: checkout_path.to_string(),
                        is_primary: !worktree.is_linked_worktree,
                        open_workspace_id: worktree.open_workspace_id.clone(),
                        track,
                        panes: Vec::new(),
                    }
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
                //
                // And no track, for the reason the arm above has none: this row names no
                // branch, so a marker on it is about a branch it cannot name. What git has
                // at this path is a `worktreepath` off a registration, which says where a
                // branch was checked out and not what is checked out there now — a
                // `git worktree add --detach` at a path some stale registration still
                // claims is both branchless and unlisted, and an unmissable `gone` beside a
                // directory name is the wrong marker rather than a late one. A missing
                // marker beats a wrong one, which is the judgement #45 makes one arm up.
                // What git said is still on `dump`'s page, under `git names at this path:`,
                // where naming it costs nothing. Issue #49.
                repo.worktrees.push(WorktreeNode {
                    branch: None,
                    checkout_path: checkout.to_string(),
                    is_primary: false,
                    open_workspace_id: Some(node.workspace_id.clone()),
                    track: None,
                    panes: vec![node],
                });
            }
        }
    }

    for repo in &mut nodes {
        // The main checkout stays first so the repository's "home" row keeps its place as
        // worktrees come and go.
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
        // What the reading could not do is not this function's to know: it is handed what
        // was read. `app::collect` records the rest onto the tree afterwards.
        trouble: Trouble::default(),
    }
}

#[cfg(test)]
mod tests;
