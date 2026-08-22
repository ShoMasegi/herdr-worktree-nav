//! What a keybinding actually runs.
//!
//! herdr invokes an action with the plugin directory as its working directory, so the action
//! is not the picker — it is the thing that opens the picker in the right place. It reads
//! `HERDR_PLUGIN_CONTEXT_JSON` to learn where the user was, and forwards what the pane
//! process cannot work out for itself.

use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::app::context::{Context, FROM_PANE, REPO_ROOT};
use crate::port::{HerdrPort, PluginPaneOpen};
use crate::PLUGIN_ID;

/// Overlay: a temporary zoom that covers the session while the picker is up and gets out of
/// the way the moment it closes.
const PLACEMENT: &str = "overlay";

/// Remembers the pane the last invocation opened, so a second press focuses it rather than
/// stacking a second overlay on top of the first.
const OPEN_PANE_FILE: &str = "open-pane";

pub fn run(herdr: &dyn HerdrPort, action_id: &str) -> Result<()> {
    let entrypoint = match action_id {
        "open-panes" => "panes",
        "open-branches" => "branches",
        other => bail!("unknown action `{other}`"),
    };
    let context = Context::from_env();

    if let Some(pane_id) = already_open(herdr)? {
        herdr.plugin_pane_focus(&pane_id)?;
        return Ok(());
    }

    let mut env = Vec::new();
    if let Some(pane_id) = &context.focused_pane_id {
        env.push((FROM_PANE.to_string(), pane_id.clone()));
    }
    if let Some(worktree) = &context.worktree {
        env.push((REPO_ROOT.to_string(), worktree.repo_root.clone()));
    }

    let pane = herdr.plugin_pane_open(&PluginPaneOpen {
        plugin_id: PLUGIN_ID.to_string(),
        entrypoint: entrypoint.to_string(),
        placement: PLACEMENT,
        // Without this the picker would start in the plugin directory and think every
        // invocation came from this repository.
        cwd: context.cwd().map(str::to_string),
        env,
        focus: true,
    })?;

    remember(&pane.pane_id);
    Ok(())
}

/// The pane a previous invocation opened, if it is still alive.
fn already_open(herdr: &dyn HerdrPort) -> Result<Option<String>> {
    let Some(path) = open_pane_path() else {
        return Ok(None);
    };
    let Ok(pane_id) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let pane_id = pane_id.trim().to_string();
    if pane_id.is_empty() {
        return Ok(None);
    }
    // The file outlives the pane, so the snapshot decides, not the file.
    let alive = herdr
        .snapshot()?
        .panes
        .iter()
        .any(|pane| pane.pane_id == pane_id);
    Ok(alive.then_some(pane_id))
}

fn remember(pane_id: &str) {
    // Best effort: failing to write the marker only means the next press opens a second
    // overlay, which is not worth failing the action over.
    if let Some(path) = open_pane_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, pane_id);
    }
}

fn open_pane_path() -> Option<PathBuf> {
    // herdr guarantees this directory for runtime state a plugin owns.
    std::env::var_os("HERDR_PLUGIN_STATE_DIR").map(|dir| PathBuf::from(dir).join(OPEN_PANE_FILE))
}
