//! The branches picker: choose a repository, choose a branch, then choose where its pane
//! goes.
//!
//! The local answer is on screen immediately and the remote is folded in when it arrives.
//! `git ls-remote` and `gh pr list` both need the network, and a picker that blocks on them
//! is a picker nobody uses offline.
//!
//! Each repository is read once. Walking back to the list and into another one is common
//! enough that re-running git every time would be felt.

use std::collections::HashMap;
use std::sync::mpsc::{self, TryRecvError};

use anyhow::{anyhow, Result};
use ratatui::crossterm::event::{self, Event};

use crate::app::collect;
use crate::app::home_dir;
use crate::domain::dest;
use crate::domain::model::{normalize_path, RepoNode};
use crate::domain::resolve::{self, BranchPlan};
use crate::port::{GhPort, GitPort, HerdrPort, Pane, PullRequest, WorktreeCreate, WorktreeOpen};
use crate::ui::branches::{self, BranchAction, BranchData, BranchesState, Choice};
use crate::ui::render;
use crate::ui::theme::Theme;

/// The remote this plugin fetches from and bases never-fetched branches on.
const REMOTE: &str = "origin";

/// How long to wait for a key before checking whether a remote listing has landed.
const TICK: std::time::Duration = std::time::Duration::from_millis(80);

/// What the picker was left wanting when it closed.
pub enum Exit {
    Closed,
    ShowPanes,
}

/// A background answer, tagged with the repository it is about: the user may well have moved
/// on to another one before it arrives.
enum Update {
    RemoteHeads {
        repo_root: String,
        heads: Vec<String>,
    },
    /// The remote listing failed — offline, no `origin`, or no credentials. The picker
    /// carries on with what git already had.
    RemoteUnavailable { repo_root: String },
    PullRequests {
        repo_root: String,
        pull_requests: Vec<PullRequest>,
    },
}

impl Update {
    fn repo_root(&self) -> &str {
        match self {
            Update::RemoteHeads { repo_root, .. }
            | Update::RemoteUnavailable { repo_root }
            | Update::PullRequests { repo_root, .. } => repo_root,
        }
    }
}

pub fn run(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    gh: &dyn GhPort,
    repo_root: Option<&str>,
    from_pane_id: Option<&str>,
    theme: &Theme,
) -> Result<Exit> {
    let (snapshot, tree) = collect::collect_tree(herdr, git)?;

    // Where the picker was summoned from, as precisely as it can be known: the checkout the
    // invoking pane is in beats the repository it belongs to, because it is the row the
    // cursor should land on.
    let from = from_pane_id
        .and_then(|pane_id| tree.find_pane(pane_id))
        .map(|(_, worktree, _)| worktree.checkout_path.clone())
        .or_else(|| repo_root.map(str::to_string));

    let mut repos = tree.repos;
    if branches::locate(&repos, from.as_deref()).is_none() {
        // A repository with no pane open is not in the tree, so fall back to a bare node:
        // its branches are still worth listing.
        if let Some(root) = repo_root {
            repos.push(bare(root));
        }
    }
    if repos.is_empty() {
        // Nothing to list branches for. The panes view is where a repository can be found.
        return Ok(Exit::ShowPanes);
    }

    let destinations = dest::destinations(&snapshot, from_pane_id);
    let mut state = BranchesState::new(repos, from.as_deref(), destinations, snapshot, home_dir());

    let (sender, receiver) = mpsc::channel();
    let (outcome, repo_root) = std::thread::scope(|scope| -> Result<(BranchAction, String)> {
        let mut cache: HashMap<String, BranchData> = HashMap::new();

        // Reading one repository: the local refs synchronously because they are a few
        // milliseconds, the remote and the pull requests on a thread because they are a
        // network round trip.
        let load = |repo_root: &str, cache: &mut HashMap<String, BranchData>| -> BranchData {
            if let Some(data) = cache.get(repo_root) {
                return data.clone();
            }
            let data = BranchData {
                local_refs: git.local_refs(repo_root).unwrap_or_default(),
                loading: true,
                ..BranchData::default()
            };
            cache.insert(repo_root.to_string(), data.clone());

            let sender = sender.clone();
            let root = repo_root.to_string();
            scope.spawn(move || {
                let _ = sender.send(match git.remote_heads(&root) {
                    Ok(heads) => Update::RemoteHeads {
                        repo_root: root.clone(),
                        heads,
                    },
                    Err(_) => Update::RemoteUnavailable {
                        repo_root: root.clone(),
                    },
                });
                let pull_requests = gh.pull_requests(&root);
                let _ = sender.send(Update::PullRequests {
                    repo_root: root,
                    pull_requests,
                });
            });
            data
        };

        // The repository the cursor starts on is read before the first frame, so opening it
        // is instant whether or not there is a repository step in front of it.
        let first = state.repo().repo_root.clone();
        let data = load(&first, &mut cache);
        state.set_data(data);

        let mut terminal = ratatui::try_init()?;
        let action = loop {
            terminal.draw(|frame| render::draw_branches(frame, &state, theme))?;

            match receiver.try_recv() {
                Ok(update) => {
                    let repo_root = update.repo_root().to_string();
                    let entry = cache.entry(repo_root.clone()).or_default();
                    match update {
                        Update::RemoteHeads { heads, .. } => {
                            entry.remote_heads = heads;
                            entry.loading = false;
                        }
                        Update::RemoteUnavailable { .. } => entry.loading = false,
                        Update::PullRequests { pull_requests, .. } => {
                            entry.pull_requests = pull_requests
                        }
                    }
                    // An answer for a repository the user has already left updates the cache
                    // and nothing else; it is there for them when they come back.
                    if state.repo().repo_root == repo_root {
                        state.set_data(entry.clone());
                    }
                    continue;
                }
                // The sender above is still held here, so the channel cannot run dry.
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
            }

            if !event::poll(TICK)? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            match state.handle_key(key) {
                BranchAction::Consumed | BranchAction::Ignored => {}
                BranchAction::LoadRepo { repo_root } => {
                    let data = load(&repo_root, &mut cache);
                    state.set_data(data);
                }
                action => break action,
            }
        };
        ratatui::try_restore()?;
        Ok((action, state.repo().repo_root.clone()))
    })?;

    perform(herdr, git, &repo_root, outcome)
}

/// A repository herdr has no worktree record for. Its branches are still listable.
fn bare(repo_root: &str) -> RepoNode {
    let repo_root = normalize_path(repo_root);
    RepoNode {
        repo_key: String::new(),
        repo_root: repo_root.to_string(),
        display_name: repo_root
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(repo_root)
            .to_string(),
        worktrees: Vec::new(),
    }
}

fn perform(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    repo_root: &str,
    action: BranchAction,
) -> Result<Exit> {
    match action {
        BranchAction::Quit => Ok(Exit::Closed),
        BranchAction::ShowPanes => Ok(Exit::ShowPanes),
        BranchAction::Jump { pane_id } => {
            herdr.pane_focus(&pane_id)?;
            Ok(Exit::Closed)
        }
        BranchAction::Chosen(choice) => {
            open(herdr, git, repo_root, *choice)?;
            Ok(Exit::Closed)
        }
        // Handled inside the loop; the picker never leaves with one of these.
        BranchAction::Consumed | BranchAction::Ignored | BranchAction::LoadRepo { .. } => {
            Ok(Exit::Closed)
        }
    }
}

/// Make the checkout exist, then put its pane where the user asked.
fn open(herdr: &dyn HerdrPort, git: &dyn GitPort, repo_root: &str, choice: Choice) -> Result<()> {
    let head = git.head_ref(repo_root)?;
    let root_pane = match resolve::plan(&choice.entry, &head, REMOTE) {
        BranchPlan::Focus { pane_id } => {
            // Reachable only if the branch started running while the picker was open.
            herdr.pane_focus(&pane_id)?;
            return Ok(());
        }
        BranchPlan::Open { checkout_path } => open_existing(herdr, repo_root, &checkout_path)?,
        BranchPlan::Create { branch, base } => create(herdr, repo_root, &branch, base)?,
        BranchPlan::FetchThenCreate { branch, base } => {
            git.fetch_branch(repo_root, &branch)?;
            create(herdr, repo_root, &branch, Some(base))?
        }
    };

    // `None` means the destination is the workspace herdr just made, so there is nothing to
    // move — only to focus.
    match dest::placement_for(&choice.destination) {
        Some(placement) => herdr.pane_move(&root_pane.pane_id, &placement, true)?,
        None => herdr.pane_focus(&root_pane.pane_id)?,
    }
    Ok(())
}

/// Create without focusing, so the workspace herdr materialises does not flash past the
/// user on its way to being moved and closed.
fn create(
    herdr: &dyn HerdrPort,
    repo_root: &str,
    branch: &str,
    base: Option<String>,
) -> Result<Pane> {
    let created = herdr.worktree_create(&WorktreeCreate {
        cwd: repo_root.to_string(),
        branch: Some(branch.to_string()),
        base,
        focus: false,
    })?;
    Ok(created.root_pane)
}

fn open_existing(herdr: &dyn HerdrPort, repo_root: &str, checkout_path: &str) -> Result<Pane> {
    let opened = herdr.worktree_open(&WorktreeOpen {
        cwd: repo_root.to_string(),
        path: Some(checkout_path.to_string()),
        branch: None,
        focus: false,
    })?;
    if opened.already_open {
        // Something else opened it between listing and choosing.
        return Err(anyhow!(
            "{checkout_path} was opened by something else while the picker was up"
        ));
    }
    Ok(opened.root_pane)
}

#[cfg(test)]
mod tests {
    use super::bare;

    #[test]
    fn a_repository_herdr_has_no_record_of_is_named_after_its_directory() {
        assert_eq!(bare("/src/app").display_name, "app");
        assert_eq!(bare("/src/app/").display_name, "app");
        assert_eq!(bare("app").display_name, "app");
    }
}
