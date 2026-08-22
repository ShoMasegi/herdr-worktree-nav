//! Wiring: the commands herdr invokes, and the impure gathering they need.

pub mod action;
pub mod branches;
pub mod collect;
pub mod context;
pub mod panes;

use anyhow::Result;

use crate::app::context::{Context, FROM_PANE, REPO_ROOT};
use crate::port::{GhPort, GitPort, HerdrPort};

/// Which picker to start on. They toggle with `Tab`, so this is only the entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entrypoint {
    Panes,
    Branches,
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
    let own_pane = std::env::var("HERDR_PANE_ID")
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

    let mut view = start;
    loop {
        match view {
            Entrypoint::Branches => {
                let Some(root) = repo_root.clone() else {
                    // Nothing to list branches for, so start where a repository can be
                    // picked instead of failing.
                    view = Entrypoint::Panes;
                    continue;
                };
                match branches::run(
                    herdr,
                    git,
                    gh,
                    &root,
                    from_pane.as_deref(),
                    own_pane.as_deref(),
                )? {
                    branches::Exit::Closed => return Ok(()),
                    branches::Exit::ShowPanes => view = Entrypoint::Panes,
                }
            }
            Entrypoint::Panes => {
                match panes::run(herdr, git, from_pane.as_deref(), own_pane.as_deref())? {
                    panes::Exit::Closed => return Ok(()),
                    panes::Exit::ShowBranches { repo_root: root } => {
                        repo_root = Some(root);
                        view = Entrypoint::Branches;
                    }
                }
            }
        }
    }
}
