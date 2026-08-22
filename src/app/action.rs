//! What a keybinding actually runs.
//!
//! herdr invokes an action with the plugin directory as its working directory, so the action
//! is not the picker — it is the thing that opens the picker in the right place. It reads
//! `HERDR_PLUGIN_CONTEXT_JSON` to learn where the user was, and forwards what the pane
//! process cannot work out for itself.

use anyhow::{bail, Result};

use crate::app::context::{Context, FROM_PANE, REPO_ROOT};
use crate::port::{HerdrPort, OpenRefusal, PluginPaneOpen};
use crate::PLUGIN_ID;

pub fn run(herdr: &dyn HerdrPort, action_id: &str) -> Result<()> {
    let entrypoint = match action_id {
        "open-panes" => "panes",
        "open-branches" => "branches",
        other => bail!("unknown action `{other}`"),
    };
    let context = Context::from_env();

    let mut env = Vec::new();
    if let Some(pane_id) = &context.focused_pane_id {
        env.push((FROM_PANE.to_string(), pane_id.clone()));
    }
    if let Some(worktree) = &context.worktree {
        env.push((REPO_ROOT.to_string(), worktree.repo_root.clone()));
    }

    let refusal = herdr.plugin_pane_open(&PluginPaneOpen {
        plugin_id: PLUGIN_ID.to_string(),
        entrypoint: entrypoint.to_string(),
        // Without this the picker would start in the plugin directory and think every
        // invocation came from this repository.
        cwd: context.cwd().map(str::to_string),
        env,
        focus: true,
    })?;

    // Nothing to do about it: while a popup is up, herdr routes every key into it, so the
    // picker the user just asked for is already the thing in front of them. Reaching here
    // at all takes a `herdr plugin action invoke` from a shell.
    match refusal {
        Some(OpenRefusal::PopupAlreadyOpen) | None => Ok(()),
    }
}
