//! The branches picker: choose a branch, then choose where its pane goes.
//!
//! The local answer is on screen immediately and the remote is folded in when it arrives.
//! `git ls-remote` and `gh pr list` both need the network, and a picker that blocks on them
//! is a picker nobody uses offline.

use std::sync::mpsc::{self, TryRecvError};

use anyhow::{anyhow, Result};
use ratatui::crossterm::event::{self, Event};

use crate::app::collect;
use crate::domain::dest;
use crate::domain::model::RepoNode;
use crate::domain::resolve::{self, BranchPlan};
use crate::port::{GhPort, GitPort, HerdrPort, Pane, PullRequest, WorktreeCreate, WorktreeOpen};
use crate::ui::branches::{BranchAction, BranchesState, Choice};
use crate::ui::render;

/// The remote this plugin fetches from and bases never-fetched branches on.
const REMOTE: &str = "origin";

/// How long to wait for a key before checking whether the remote listing has landed.
const TICK: std::time::Duration = std::time::Duration::from_millis(80);

/// What the picker was left wanting when it closed.
pub enum Exit {
    Closed,
    ShowPanes,
}

enum Update {
    RemoteHeads(Vec<String>),
    /// The remote listing failed — offline, no `origin`, or no credentials. The picker
    /// carries on with what git already had.
    RemoteUnavailable,
    PullRequests(Vec<PullRequest>),
}

pub fn run(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    gh: &dyn GhPort,
    repo_root: &str,
    from_pane_id: Option<&str>,
    own_pane_id: Option<&str>,
) -> Result<Exit> {
    let (snapshot, tree) = collect::collect_tree(herdr, git, own_pane_id)?;
    let repo = tree
        .repos
        .iter()
        .find(|repo| repo.repo_root == repo_root)
        .cloned()
        // A repository with no pane open is not in the tree, so fall back to a bare node:
        // its branches are still worth listing.
        .unwrap_or_else(|| RepoNode {
            repo_key: String::new(),
            repo_root: repo_root.to_string(),
            display_name: repo_root
                .rsplit('/')
                .next()
                .unwrap_or(repo_root)
                .to_string(),
            worktrees: Vec::new(),
        });

    let local_refs = git.local_refs(repo_root)?;
    let destinations = dest::destinations(&snapshot, from_pane_id, own_pane_id);
    let mut state = BranchesState::new(repo, local_refs, destinations);

    let (sender, receiver) = mpsc::channel();
    let outcome = std::thread::scope(|scope| -> Result<BranchAction> {
        scope.spawn({
            let sender = sender.clone();
            move || {
                let _ = sender.send(match git.remote_heads(repo_root) {
                    Ok(heads) => Update::RemoteHeads(heads),
                    Err(_) => Update::RemoteUnavailable,
                });
                let _ = sender.send(Update::PullRequests(gh.pull_requests(repo_root)));
            }
        });

        let mut terminal = ratatui::try_init()?;
        let action = loop {
            terminal.draw(|frame| render::draw_branches(frame, &state))?;

            match receiver.try_recv() {
                Ok(Update::RemoteHeads(heads)) => {
                    state.set_remote_heads(heads);
                    continue;
                }
                Ok(Update::RemoteUnavailable) => {
                    state.finish_loading();
                    continue;
                }
                Ok(Update::PullRequests(pull_requests)) => {
                    state.set_pull_requests(pull_requests);
                    continue;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => state.finish_loading(),
            }

            if !event::poll(TICK)? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            match state.handle_key(key) {
                BranchAction::Consumed | BranchAction::Ignored => {}
                action => break action,
            }
        };
        ratatui::try_restore()?;
        Ok(action)
    })?;

    perform(herdr, git, repo_root, outcome)
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
        BranchAction::Consumed | BranchAction::Ignored => Ok(Exit::Closed),
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
