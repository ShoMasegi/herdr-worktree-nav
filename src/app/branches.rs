//! The branches picker: choose a repository, choose a branch, then choose where its pane
//! goes.
//!
//! The local answer is on screen immediately and the remote is folded in when it arrives.
//! `git ls-remote` and `gh pr list` both need the network, and a picker that blocks on them
//! is a picker nobody uses offline.
//!
//! Each repository's remote is asked once and remembered for as long as the picker is up —
//! `Tab` to the panes view and back included, which is why the cache is owned by the caller.
//! Walking between repositories, and between the two views, is common enough that a round
//! trip in front of every frame would be felt.

use std::collections::HashMap;
use std::sync::mpsc::{self, TryRecvError};

use anyhow::{anyhow, Result};
use ratatui::crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::app::collect;
use crate::app::home_dir;
use crate::app::Summoned;
use crate::domain::dest;
use crate::domain::listing;
use crate::domain::model::{normalize_path, RepoNode};
use crate::domain::progress::Stage;
use crate::domain::resolve::{self, BranchPlan};
use crate::port::{
    GhPort, GitPort, GitRef, HerdrPort, Pane, PullRequest, WorktreeCreate, WorktreeOpen,
};
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
    /// A `git fetch` of the whole repository finished.
    Fetched {
        repo_root: String,
        result: Result<()>,
    },
    /// The chosen branch is being opened, and this is the step it has reached.
    Working(Stage),
    /// Opening finished. An error stays on screen rather than being printed after the
    /// popup has already closed over it.
    Done(Result<()>),
}

impl Update {
    /// The repository a listing answer is about. `None` for the worker's own traffic, which
    /// is always about the branch the user just chose.
    fn repo_root(&self) -> Option<&str> {
        match self {
            Update::RemoteHeads { repo_root, .. }
            | Update::RemoteUnavailable { repo_root }
            | Update::PullRequests { repo_root, .. }
            | Update::Fetched { repo_root, .. } => Some(repo_root),
            Update::Working(_) | Update::Done(_) => None,
        }
    }
}

pub fn run(
    terminal: &mut DefaultTerminal,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    gh: &dyn GhPort,
    summoned: &Summoned,
    theme: &Theme,
    listings: &mut listing::Cache,
) -> Result<Exit> {
    let (snapshot, tree) = collect::collect_tree(herdr, git)?;
    let from_pane_id = summoned.pane.as_deref();
    let repo_root = summoned.repo_root.as_deref();

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
    let (outcome, failure) =
        std::thread::scope(|scope| -> Result<(BranchAction, Option<anyhow::Error>)> {
            let mut cache: HashMap<String, BranchData> = HashMap::new();
            // Shown in the picker and then raised again on the way out, so that what the
            // user read is also what `herdr plugin log list` has.
            let mut failure: Option<anyhow::Error> = None;

            // Reading one repository: the local refs synchronously because they are a few
            // milliseconds, the remote and the pull requests on a thread because they are a
            // network round trip.
            //
            // A remote an earlier visit already heard back from is not asked again. That is
            // what `listings` is carried across a view switch for: `Tab` away and back is a
            // frame, not a round trip.
            let read = |repo_root: &str, listings: &mut listing::Cache| -> BranchData {
                let local_refs = git.local_refs(repo_root).unwrap_or_default();
                if let Some(remote) = listings.get(repo_root).filter(|remote| !remote.loading) {
                    return shown(local_refs, remote.clone());
                }

                let waiting = listing::starting(listings, repo_root);
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
                shown(local_refs, waiting)
            };
            let load = |repo_root: &str,
                        cache: &mut HashMap<String, BranchData>,
                        listings: &mut listing::Cache|
             -> BranchData {
                if let Some(data) = cache.get(repo_root) {
                    return data.clone();
                }
                let data = read(repo_root, listings);
                cache.insert(repo_root.to_string(), data.clone());
                data
            };

            // The repository the cursor starts on is read before the first frame, so opening it
            // is instant whether or not there is a repository step in front of it.
            let first = state.repo().repo_root.clone();
            let data = load(&first, &mut cache, listings);
            state.set_data(data);

            // The spinner turns on a clock rather than on redraws, so that it neither
            // speeds up while the user types nor stalls while they hold a key down.
            let mut last_tick = std::time::Instant::now();
            let action = loop {
                if last_tick.elapsed() >= TICK {
                    state.tick();
                    last_tick = std::time::Instant::now();
                }
                terminal.draw(|frame| render::draw_branches(frame, &state, theme))?;

                match receiver.try_recv() {
                    Ok(Update::Fetched { repo_root, result }) => {
                        match result {
                            // Everything the fetch wrote is in the refs, so the answer is
                            // to read the repository again rather than to patch what is on
                            // screen. It also has to be: `--prune` deleted refs, and the
                            // remote listing cached here would put them straight back.
                            Ok(()) => {
                                listing::apply(listings, &repo_root, listing::Answer::Refetched);
                                let fresh = read(&repo_root, listings);
                                cache.insert(repo_root.clone(), fresh.clone());
                                if state.repo().repo_root == repo_root {
                                    state.set_data(fresh);
                                }
                            }
                            Err(error) => {
                                let entry = cache.entry(repo_root.clone()).or_default();
                                entry.fetching = false;
                                let data = entry.clone();
                                if state.repo().repo_root == repo_root {
                                    state.set_data(data);
                                    state.set_message(format!("{error:#}"));
                                }
                            }
                        }
                        continue;
                    }
                    Ok(Update::Working(stage)) => {
                        state.set_stage(stage);
                        continue;
                    }
                    // Everything asked for has happened; the pane is where the user wanted it.
                    Ok(Update::Done(Ok(()))) => break BranchAction::Quit,
                    Ok(Update::Done(Err(error))) => {
                        state.fail(format!("{error:#}"));
                        failure = Some(error);
                        continue;
                    }
                    Ok(update) => {
                        let repo_root = update.repo_root().unwrap_or_default().to_string();
                        if let Some(remote) = listing::apply(listings, &repo_root, answer(update)) {
                            let entry = cache.entry(repo_root.clone()).or_default();
                            entry.remote_heads = remote.heads;
                            entry.pull_requests = remote.pull_requests;
                            entry.loading = remote.loading;
                            // An answer for a repository the user has already left updates the
                            // cache and nothing else; it is there for them when they come back.
                            if state.repo().repo_root == repo_root {
                                state.set_data(entry.clone());
                            }
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
                        let data = load(&repo_root, &mut cache, listings);
                        state.set_data(data);
                    }
                    BranchAction::Fetch { repo_root } => {
                        let entry = cache.entry(repo_root.clone()).or_default();
                        // A second fetch of the same repository would only race the first.
                        if !entry.fetching {
                            entry.fetching = true;
                            let data = entry.clone();
                            state.set_data(data);
                            let sender = sender.clone();
                            let root = repo_root.clone();
                            scope.spawn(move || {
                                let result = git.fetch_all(&root);
                                let _ = sender.send(Update::Fetched {
                                    repo_root: root,
                                    result,
                                });
                            });
                        }
                    }
                    // The picker stays up and becomes a progress display. Breaking out here
                    // instead would restore the terminal and leave herdr's popup framing an
                    // empty box for as long as the fetch and the checkout take.
                    BranchAction::Chosen(choice) => {
                        state.start_working(Stage::Starting {
                            branch: choice.entry.name.clone(),
                        });
                        let sender = sender.clone();
                        let repo_root = state.repo().repo_root.clone();
                        scope.spawn(move || {
                            let report = |stage: Stage| {
                                let _ = sender.send(Update::Working(stage));
                            };
                            let result = open(herdr, git, &repo_root, *choice, &report);
                            let _ = sender.send(Update::Done(result));
                        });
                    }
                    // `thread::scope` joins every background thread before this function
                    // can return, and a fetch can be a long way from done. Nothing the
                    // picker does on the way out is worth that wait, so leave the process
                    // instead. It is safe for the same reason in both cases: a `git fetch`
                    // that outlives us only ever writes `refs/remotes`, and a step that
                    // could leave something half-made does not offer Ctrl-C at all — see
                    // `Stage::interruptible`.
                    //
                    // A failure is the exception: it is raised again on the way out so that
                    // `herdr plugin log list` gets it. By then the work has finished, so
                    // the join costs nothing.
                    BranchAction::Quit if failure.is_none() => {
                        ratatui::try_restore()?;
                        std::process::exit(0);
                    }
                    action => break action,
                }
            };
            Ok((action, failure))
        })?;

    // `thread::scope` has joined every thread this view spawned, so whatever is still in the
    // channel is a finished answer that landed after the last frame. Folding it in here is
    // what makes coming back to this view a frame rather than another round trip — and it is
    // what keeps a cached entry from being left claiming it is still loading.
    while let Ok(update) = receiver.try_recv() {
        let Some(repo_root) = update.repo_root().map(str::to_string) else {
            continue;
        };
        let answer = match update {
            Update::Fetched { result: Ok(()), .. } => listing::Answer::Refetched,
            // A fetch that failed changed nothing, and the reason it gave belongs to a screen
            // that has already been drawn over.
            Update::Fetched { .. } => continue,
            update => answer(update),
        };
        listing::apply(listings, &repo_root, answer);
    }

    if let Some(error) = failure {
        return Err(error);
    }
    perform(herdr, outcome)
}

/// One repository as the picker draws it: the local refs it just read, and whatever it
/// remembers about the remote.
fn shown(local_refs: Vec<GitRef>, remote: listing::Remote) -> BranchData {
    BranchData {
        local_refs,
        remote_heads: remote.heads,
        pull_requests: remote.pull_requests,
        loading: remote.loading,
        fetching: false,
    }
}

/// The listing answer an update carries. `Fetched` is not one — it is a repository to forget
/// rather than a fact to record — and the worker's own traffic is about the branch being
/// opened, not about a listing; both are sorted out before this is reached.
fn answer(update: Update) -> listing::Answer {
    match update {
        Update::RemoteHeads { heads, .. } => listing::Answer::Heads(heads),
        Update::RemoteUnavailable { .. } => listing::Answer::Unavailable,
        Update::PullRequests { pull_requests, .. } => listing::Answer::PullRequests(pull_requests),
        Update::Working(_) | Update::Done(_) | Update::Fetched { .. } => {
            unreachable!("handled above")
        }
    }
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

fn perform(herdr: &dyn HerdrPort, action: BranchAction) -> Result<Exit> {
    match action {
        BranchAction::Quit => Ok(Exit::Closed),
        BranchAction::ShowPanes => Ok(Exit::ShowPanes),
        BranchAction::Jump { pane_id } => {
            herdr.pane_focus(&pane_id)?;
            Ok(Exit::Closed)
        }
        // Opening happens on the worker thread while the picker is still up, so the
        // picker never leaves with one of these.
        BranchAction::Chosen(_)
        | BranchAction::Consumed
        | BranchAction::Ignored
        | BranchAction::LoadRepo { .. }
        | BranchAction::Fetch { .. } => Ok(Exit::Closed),
    }
}

/// Make the checkout exist, then put its pane where the user asked.
///
/// Runs on a worker thread and announces each step through `report`, because the two slow
/// ones — a fetch across the network and a checkout of a whole working tree — are long
/// enough that silence reads as a crash.
fn open(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    repo_root: &str,
    choice: Choice,
    report: &dyn Fn(Stage),
) -> Result<()> {
    let head = git.head_ref(repo_root)?;
    let root_pane = match resolve::plan(&choice.entry, &head, REMOTE) {
        BranchPlan::Focus { pane_id } => {
            // Reachable only if the branch started running while the picker was open.
            herdr.pane_focus(&pane_id)?;
            return Ok(());
        }
        BranchPlan::Open { checkout_path } => {
            report(Stage::Opening {
                branch: choice.entry.name.clone(),
            });
            open_existing(herdr, repo_root, &checkout_path)?
        }
        BranchPlan::Create { branch, base } => {
            report(Stage::Creating {
                branch: branch.clone(),
            });
            create(herdr, repo_root, &branch, base)?
        }
        BranchPlan::FetchThenCreate { branch, base } => {
            report(Stage::Fetching {
                remote: REMOTE.to_string(),
                branch: branch.clone(),
            });
            git.fetch_branch(repo_root, &branch)?;
            report(Stage::Creating {
                branch: branch.clone(),
            });
            create(herdr, repo_root, &branch, Some(base))?
        }
    };

    report(Stage::Landing {
        destination: choice.destination.clone(),
    });
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
