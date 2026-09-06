//! Wiring: the commands herdr invokes, and the impure gathering they need.

pub mod action;
pub mod branches;
pub mod collect;
pub mod context;
pub mod dirty;
#[cfg(test)]
pub(crate) mod fakes;
pub mod panes;
pub mod removals;
pub mod remove;
pub mod settled;

use anyhow::Result;
use ratatui::DefaultTerminal;

use std::sync::Arc;

use crate::adapter::herdr_config;
use crate::app::context::{Context, FROM_PANE, REPO_ROOT};
use crate::app::dirty::Dirty;
use crate::app::removals::Removals;
use crate::app::settled::Settled;
use crate::domain::listing;
use crate::port::{GhPort, GitPort, HerdrPort, RemovalPort};
use crate::ui::theme::Theme;

/// What the panes view has asked for behind the first frame.
///
/// Kept across a `Tab` and across leaving a sweep, so that a trip through the branches view
/// asks none of it again — an answer that cost a round of processes is worth more than the
/// frame it took to get. Both are here rather than passed separately because they are the
/// same shape: a question put to a port on a thread, drained by the loop, and drawn on the
/// rows as it lands.
pub struct Pending {
    /// Which checkouts are holding uncommitted work. One process per checkout, so it is
    /// asked as soon as the picker opens and fills the list in behind the first frame.
    pub dirty: Dirty,
    /// What `gh` says has become of each repository's pull requests. Asked when a sweep is
    /// entered rather than when the picker opens: it is the heavier call, and most sessions
    /// never sweep — `docs/adr/0011-what-may-be-swept.md`.
    pub settled: Settled,
}

/// Which picker to start on. They toggle with `Tab`, so this is only the entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entrypoint {
    Panes,
    Branches,
}

/// Where the picker was summoned from, as precisely as the action could tell it.
///
/// `repo_root` is not fixed for the life of the picker: leaving the panes view carries the
/// repository under the cursor into the branches view.
pub struct Summoned {
    pub pane: Option<String>,
    pub repo_root: Option<String>,
}

/// The user's home directory, for shortening checkout paths in the lists.
pub(crate) fn home_dir() -> Option<String> {
    dirs::home_dir().and_then(|home| home.to_str().map(str::to_string))
}

/// Run the picker until the user leaves it, switching views as they ask.
pub fn run_picker(
    herdr: &dyn HerdrPort,
    git: Arc<dyn GitPort>,
    gh: Arc<dyn GhPort>,
    remover: &dyn RemovalPort,
    start: Entrypoint,
) -> Result<()> {
    // The action that opened this pane forwarded where it was summoned from; `HERDR_PANE_ID`
    // in a pane process is this pane's own id, which is not the same thing.
    let from_pane = std::env::var(FROM_PANE)
        .ok()
        .filter(|value| !value.is_empty());
    let mut repo_root = std::env::var(REPO_ROOT).ok().filter(|v| !v.is_empty());
    if repo_root.is_none() {
        // Fall back to the directory herdr started this pane in, which the action set to
        // wherever the user was.
        let cwd = Context::from_env()
            .cwd()
            .map(str::to_string)
            .or_else(|| std::env::current_dir().ok()?.to_str().map(str::to_string));
        repo_root = cwd
            .and_then(|cwd| git.identify(&cwd).ok().flatten())
            .map(|identity| identity.checkout_path);
    }

    // Borrowed from herdr's own configuration so the pickers look like its navigator
    // rather than like a different program.
    let theme = Theme::new(herdr_config::load());
    let summoned = Summoned {
        pane: from_pane,
        repo_root,
    };

    // The picker owns the terminal for as long as it is up, and the views borrow it. Putting
    // it back between them would leave herdr's popup framing the empty primary screen for
    // however long the next view takes to gather what it draws — see
    // `docs/adr/0009-the-picker-owns-the-terminal.md`.
    let mut terminal = ratatui::try_init()?;
    let result = views(
        &mut terminal,
        herdr,
        git,
        gh,
        remover,
        &theme,
        start,
        summoned,
    );
    // Restoring on every path out, including the ones `?` takes inside `views`. A loop that
    // failed says more than a terminal that would not go back, so it wins.
    let restored = ratatui::try_restore();
    result.and(restored.map_err(Into::into))
}

/// Switch between the two views until the user leaves the picker.
#[allow(clippy::too_many_arguments)]
fn views(
    terminal: &mut DefaultTerminal,
    herdr: &dyn HerdrPort,
    git: Arc<dyn GitPort>,
    gh: Arc<dyn GhPort>,
    remover: &dyn RemovalPort,
    theme: &Theme,
    start: Entrypoint,
    mut summoned: Summoned,
) -> Result<()> {
    // What each repository's remote answered, kept across the switch. Re-reading it every
    // time `Tab` came back would be a network round trip in front of every frame.
    let mut listings = listing::Cache::new();
    // Removals in flight, kept across the switch for the same reason and one more: they
    // outlive the picker entirely, so the view that started one is not necessarily the view
    // that is up when it finishes.
    let mut removals = Removals::new(remover);
    // Kept for the same reason, and with one of its own: walking a working tree to see
    // whether it is dirty is the one answer here that costs a process per checkout.
    let mut pending = Pending {
        dirty: Dirty::new(Arc::clone(&git)),
        settled: Settled::new(Arc::clone(&git), Arc::clone(&gh)),
    };
    let mut view = start;
    loop {
        match view {
            Entrypoint::Branches => {
                // No repository in hand is not a failure: the picker opens on its list of
                // them. It falls back to the panes view only when there are none at all.
                match branches::run(
                    terminal,
                    herdr,
                    &*git,
                    &*gh,
                    &summoned,
                    theme,
                    &mut listings,
                )? {
                    branches::Exit::Closed => return Ok(()),
                    branches::Exit::ShowPanes => view = Entrypoint::Panes,
                }
            }
            Entrypoint::Panes => {
                match panes::run(
                    terminal,
                    herdr,
                    &*git,
                    &mut removals,
                    &mut pending,
                    summoned.pane.as_deref(),
                    theme,
                )? {
                    panes::Exit::Closed => return Ok(()),
                    panes::Exit::ShowBranches { repo_root } => {
                        // `None` when the cursor was not in a repository; the branches picker
                        // then simply starts with nothing preselected.
                        summoned.repo_root = repo_root;
                        view = Entrypoint::Branches;
                    }
                }
            }
        }
    }
}
