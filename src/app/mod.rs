//! Wiring: the commands herdr invokes, and the impure gathering they need.

pub mod action;
pub mod branches;
pub mod collect;
pub mod context;
pub mod panes;

use anyhow::Result;

use crate::adapter::herdr_config;
use crate::app::context::{Context, FROM_PANE, REPO_ROOT};
use crate::port::{GhPort, GitPort, HerdrPort};
use crate::ui::theme::Theme;

/// Which picker to start on. They toggle with `Tab`, so this is only the entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entrypoint {
    Panes,
    Branches,
}

/// The user's home directory, for shortening checkout paths in the lists.
pub(crate) fn home_dir() -> Option<String> {
    dirs::home_dir().and_then(|home| home.to_str().map(str::to_string))
}

/// Run the picker until the user leaves it, switching views as they ask.
pub fn run_picker(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    gh: &dyn GhPort,
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

    let mut view = start;
    loop {
        match view {
            Entrypoint::Branches => {
                // No repository in hand is not a failure: the picker opens on its list of
                // them. It falls back to the panes view only when there are none at all.
                match branches::run(
                    herdr,
                    git,
                    gh,
                    repo_root.as_deref(),
                    from_pane.as_deref(),
                    &theme,
                )? {
                    branches::Exit::Closed => return Ok(()),
                    branches::Exit::ShowPanes => view = Entrypoint::Panes,
                }
            }
            Entrypoint::Panes => match panes::run(herdr, git, from_pane.as_deref(), &theme)? {
                panes::Exit::Closed => return Ok(()),
                panes::Exit::ShowBranches { repo_root: root } => {
                    // `None` when the cursor was not in a repository; the branches picker
                    // then simply starts with nothing preselected.
                    repo_root = root;
                    view = Entrypoint::Branches;
                }
            },
        }
    }
}
