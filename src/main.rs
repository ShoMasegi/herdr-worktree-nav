//! herdr launches this binary two ways:
//!
//! - `action <id>` — a keybinding fired a plugin action. Opens the picker where the user is.
//! - `pane <entrypoint>` — herdr is starting the plugin pane itself. Runs the picker.
//!
//! `dump` is a third, diagnostic mode for troubleshooting what the plugin sees.
//!
//! `remove` is the fourth, and the only one herdr does not start: the picker starts it, in
//! a session of its own, so that deleting a checkout outlives the window that asked for it.
//! See `docs/adr/0014-removing-outlives-the-picker.md`.

use std::process::ExitCode;

use anyhow::{bail, Result};
use herdr_worktree_nav::adapter::{herdr_config, DetachedRemovals, GhCli, GitCli, SocketHerdr};
use herdr_worktree_nav::app::{action, collect, dump, remove, run_picker, Entrypoint};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // herdr surfaces a failed plugin command's stderr in `herdr plugin log list`.
            eprintln!("herdr-worktree-nav: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("action") => match args.next().as_deref() {
            Some(id) => action::run(&SocketHerdr::from_env()?, id),
            None => bail!("`action` needs an action id"),
        },
        Some("pane") => match args.next().as_deref() {
            Some("panes") => pane(Entrypoint::Panes),
            Some("branches") => pane(Entrypoint::Branches),
            Some(other) => bail!("unknown pane entrypoint `{other}`"),
            None => bail!("`pane` needs an entrypoint: `panes` or `branches`"),
        },
        Some("dump") => dump(),
        Some("remove") => {
            let args = remove::Args::read(&mut args)?;
            remove::run(
                &SocketHerdr::from_env()?,
                &GitCli,
                &args.repo_root,
                &args.checkout_path,
                &args.label,
                args.panes_closed,
            )
        }
        Some(other) => {
            bail!("unknown command `{other}`. Expected `action`, `pane`, `dump`, or `remove`.")
        }
        None => {
            eprintln!("{USAGE}");
            bail!("no command given")
        }
    }
}

const USAGE: &str = "\
herdr-worktree-nav — navigate herdr panes by repo and worktree

  herdr-worktree-nav action <action-id>   open the picker for a plugin action (herdr calls this)
  herdr-worktree-nav pane <entrypoint>    run the picker itself (herdr calls this)
  herdr-worktree-nav dump                 print what the plugin currently sees, for troubleshooting
  herdr-worktree-nav remove <repo-root> <checkout-path> <branch> [panes-closed]
                                          remove one checkout and say so (the picker calls this)";

fn pane(start: Entrypoint) -> Result<()> {
    run_picker(
        &SocketHerdr::from_env()?,
        // Shared rather than borrowed: the threads asking whether each checkout is dirty
        // outlive the view that started them, so they cannot borrow from one.
        std::sync::Arc::new(GitCli),
        // Shared for the same reason: the sweep asks `gh` about each repository on a thread
        // that outlives the view that entered it.
        std::sync::Arc::new(GhCli),
        &DetachedRemovals,
        start,
    )
}

/// Print what the plugin sees as plain text. Useful when the picker shows something
/// surprising: it separates "herdr or git told us something odd" from "the UI drew it wrong".
fn dump() -> Result<()> {
    let herdr = SocketHerdr::from_env()?;
    let (snapshot, tree) = collect::collect_tree(&herdr, &GitCli)?;

    print!(
        "{}",
        dump::report(
            &snapshot,
            &herdr_config::load(),
            &tree,
            &dump::read_refs(&GitCli, &tree),
            &dump::read_working_trees(&GitCli, &tree),
        )
    );
    Ok(())
}
