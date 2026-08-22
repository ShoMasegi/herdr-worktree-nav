//! Gathering the inputs `domain::tree::build` needs.
//!
//! This is the impure half of building the panes view: it asks herdr for the session, works
//! out which repository and checkout every pane is in, and asks herdr for each repository's
//! worktrees. The decisions all live in `domain`; this module only fetches.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;

use crate::domain::model::{normalize_path, Tree};
use crate::domain::tree::{self, PanePlacement, RepoInput};
use crate::port::{GitPort, HerdrPort, Snapshot};

/// Fetch everything and build the tree.
pub fn collect_tree(herdr: &dyn HerdrPort, git: &dyn GitPort) -> Result<(Snapshot, Tree)> {
    let snapshot = herdr.snapshot()?;
    let placements = resolve_placements(&snapshot, git);
    let repos = collect_repos(herdr, git, &placements);
    let tree = tree::build(&snapshot, &repos, &placements);
    Ok((snapshot, tree))
}

/// Work out which repository and checkout each pane sits in.
///
/// Two shortcuts keep this cheap. Panes are resolved per working directory rather than per
/// pane, because several panes usually share one; and when herdr already knows a workspace
/// is a worktree, its answer is reused instead of running git — but only for panes that are
/// still somewhere under that checkout, since a pane is free to `cd` into another repository.
fn resolve_placements(snapshot: &Snapshot, git: &dyn GitPort) -> HashMap<String, PanePlacement> {
    let workspace_worktrees: HashMap<&str, PanePlacement> = snapshot
        .workspaces
        .iter()
        .filter_map(|workspace| {
            workspace.worktree.as_ref().map(|worktree| {
                (
                    workspace.workspace_id.as_str(),
                    PanePlacement {
                        repo_key: normalize_path(&worktree.repo_key).to_string(),
                        checkout_path: normalize_path(&worktree.checkout_path).to_string(),
                    },
                )
            })
        })
        .collect();

    let mut placements = HashMap::new();
    let mut unresolved: HashSet<&str> = HashSet::new();

    for pane in &snapshot.panes {
        let Some(cwd) = pane.effective_cwd() else {
            continue;
        };
        match workspace_worktrees.get(pane.workspace_id.as_str()) {
            Some(known) if is_inside(cwd, &known.checkout_path) => {
                placements.insert(pane.pane_id.clone(), known.clone());
            }
            _ => {
                unresolved.insert(cwd);
            }
        }
    }

    let resolved = identify_all(git, &unresolved);

    for pane in &snapshot.panes {
        if placements.contains_key(&pane.pane_id) {
            continue;
        }
        let Some(cwd) = pane.effective_cwd() else {
            continue;
        };
        if let Some(Some(placement)) = resolved.get(cwd) {
            placements.insert(pane.pane_id.clone(), placement.clone());
        }
    }

    placements
}

/// Resolve every distinct working directory, several at a time.
///
/// A `git rev-parse` is a few milliseconds; a user with many panes open across many
/// repositories would feel them added up, and the picker has to open instantly.
fn identify_all<'a>(
    git: &dyn GitPort,
    cwds: &HashSet<&'a str>,
) -> BTreeMap<&'a str, Option<PanePlacement>> {
    /// Enough to hide the latency without flooding a laptop with git processes.
    const MAX_IN_FLIGHT: usize = 8;

    let cwds: Vec<&str> = cwds.iter().copied().collect();
    let mut resolved = BTreeMap::new();

    for chunk in cwds.chunks(MAX_IN_FLIGHT) {
        let results: Vec<Option<PanePlacement>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|cwd| scope.spawn(move || identify_one(git, cwd)))
                .collect();
            handles
                .into_iter()
                // A panicking git resolution must not take the picker down with it; the
                // pane just ends up ungrouped.
                .map(|handle| handle.join().unwrap_or(None))
                .collect()
        });
        for (cwd, placement) in chunk.iter().zip(results) {
            resolved.insert(*cwd, placement);
        }
    }
    resolved
}

fn identify_one(git: &dyn GitPort, cwd: &str) -> Option<PanePlacement> {
    // A path that is not in a repository, and a git that failed, are the same thing here:
    // the pane is simply not grouped.
    let identity = git.identify(cwd).ok().flatten()?;
    Some(PanePlacement {
        repo_key: normalize_path(&identity.repo_key).to_string(),
        checkout_path: normalize_path(&identity.checkout_path).to_string(),
    })
}

/// Whether `path` is `root` or sits underneath it.
fn is_inside(path: &str, root: &str) -> bool {
    let path = normalize_path(path);
    let root = normalize_path(root);
    path == root || path.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
}

/// Ask herdr for the worktrees of every repository a pane was found in.
fn collect_repos(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    placements: &HashMap<String, PanePlacement>,
) -> Vec<RepoInput> {
    // One checkout per repo is enough to ask about: herdr resolves the whole repository
    // from any path inside it. BTreeMap keeps the result deterministic.
    let mut probe_paths: BTreeMap<&str, &str> = BTreeMap::new();
    for placement in placements.values() {
        probe_paths
            .entry(&placement.repo_key)
            .or_insert(&placement.checkout_path);
    }

    probe_paths
        .into_iter()
        .filter_map(|(repo_key, probe)| {
            let listed = herdr.worktree_list(probe).ok()?;
            let repo_root = normalize_path(&listed.source.repo_root).to_string();
            let display_name = git
                .github_slug(&repo_root)
                .ok()
                .flatten()
                .unwrap_or_else(|| listed.source.repo_name.clone());
            Some(RepoInput {
                repo_key: repo_key.to_string(),
                repo_root,
                display_name,
                worktrees: listed.worktrees,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::is_inside;

    #[test]
    fn recognises_a_pane_that_is_still_inside_its_workspace_checkout() {
        assert!(is_inside("/src/app", "/src/app"));
        assert!(is_inside("/src/app/", "/src/app"));
        assert!(is_inside("/src/app/src/ui", "/src/app"));
    }

    #[test]
    fn rejects_a_sibling_directory_that_merely_shares_a_prefix() {
        // The case that a naive starts_with would get wrong.
        assert!(!is_inside("/src/app-tools", "/src/app"));
        assert!(!is_inside("/src/other", "/src/app"));
        assert!(!is_inside("/src", "/src/app"));
    }
}
