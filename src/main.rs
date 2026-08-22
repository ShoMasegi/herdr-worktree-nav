//! herdr launches this binary two ways:
//!
//! - `action <id>` — a keybinding fired a plugin action. Opens the matching pane.
//! - `pane <entrypoint>` — herdr is starting the plugin pane itself. Runs the picker.
//!
//! `dump` is a third, diagnostic mode for troubleshooting what the plugin sees.

use std::process::ExitCode;

use anyhow::{bail, Result};
use herdr_gh_nav::adapter::{GitCli, SocketHerdr};
use herdr_gh_nav::app::{collect, panes};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // herdr surfaces a failed plugin command's stderr in `herdr plugin log list`.
            eprintln!("herdr-gh-nav: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("dump") => dump(),
        Some("pane") => pane(args.next().as_deref()),
        Some("action") => bail!("not implemented yet"),
        Some(other) => bail!("unknown command `{other}`. Expected `action`, `pane`, or `dump`."),
        None => {
            eprintln!("{}", USAGE);
            bail!("no command given")
        }
    }
}

const USAGE: &str = "\
herdr-gh-nav — navigate herdr panes by repo and worktree

  herdr-gh-nav action <action-id>   open the picker for a plugin action (herdr calls this)
  herdr-gh-nav pane <entrypoint>    run the picker itself (herdr calls this)
  herdr-gh-nav dump                 print what the plugin currently sees, for troubleshooting";

/// herdr is starting one of the manifest's pane entrypoints.
fn pane(entrypoint: Option<&str>) -> Result<()> {
    let herdr = SocketHerdr::from_env()?;
    match entrypoint {
        Some("panes") => {
            panes::run(
                &herdr,
                &GitCli,
                std::env::var("HERDR_PANE_ID").ok().as_deref(),
            )?;
            Ok(())
        }
        Some("branches") => bail!("the branches view is not implemented yet"),
        Some(other) => bail!("unknown pane entrypoint `{other}`"),
        None => bail!("`pane` needs an entrypoint: `panes` or `branches`"),
    }
}

/// Print the resolved tree as plain text. Useful when the picker shows something surprising:
/// it separates "herdr/git told us something odd" from "the UI drew it wrong".
fn dump() -> Result<()> {
    let herdr = SocketHerdr::from_env()?;
    let (snapshot, tree) = collect::collect_tree(&herdr, &GitCli)?;

    println!(
        "herdr {} (protocol {})",
        snapshot.version, snapshot.protocol
    );
    println!(
        "{} panes in {} repos",
        snapshot.panes.len(),
        tree.repos.len()
    );
    for repo in &tree.repos {
        println!("\n{}  [{}]", repo.display_name, repo.repo_root);
        for worktree in &repo.worktrees {
            let open = match &worktree.open_workspace_id {
                Some(id) => format!(" open in {id}"),
                None => String::new(),
            };
            println!(
                "  {} {}{}  {}",
                if worktree.is_primary { "*" } else { "-" },
                worktree.label(),
                open,
                worktree.checkout_path
            );
            for pane in &worktree.panes {
                println!(
                    "      {}  {:?}  {}",
                    pane.pane_id,
                    pane.agent_status,
                    pane.display_name.as_deref().unwrap_or("")
                );
            }
        }
    }
    if !tree.ungrouped.is_empty() {
        println!("\nnot in any repository:");
        for pane in &tree.ungrouped {
            println!("      {}", pane.pane_id);
        }
    }
    Ok(())
}
