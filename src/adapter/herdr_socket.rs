//! `HerdrPort` over herdr's Unix socket API.
//!
//! The socket is the transport rather than the `herdr` CLI for one decisive reason:
//! `pane.focus` — jumping to a chosen pane, which is the whole point of the panes view —
//! has no CLI equivalent. The CLI's `pane focus` is directional and `agent focus` rejects
//! panes with no agent. See `docs/adr/0002-socket-transport.md`.
//!
//! Protocol: connect, write one JSON request terminated by a newline, read one JSON
//! response terminated by a newline. The server closes the connection after replying, so
//! every call gets its own connection. Connecting to a Unix socket is cheap enough that
//! this costs less than spawning the CLI would.

use std::cell::Cell;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::port::*;

/// Long enough to cover a `worktree.create` that has to check out a large repository,
/// short enough that a wedged server surfaces as an error rather than a hung picker.
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct SocketHerdr {
    socket_path: PathBuf,
    next_id: Cell<u64>,
}

impl SocketHerdr {
    /// Locate the socket the way herdr tells plugins to: `HERDR_SOCKET_PATH`, which herdr
    /// injects into every plugin command.
    pub fn from_env() -> Result<Self> {
        let path = std::env::var_os("HERDR_SOCKET_PATH").ok_or_else(|| {
            anyhow!(
                "HERDR_SOCKET_PATH is not set. herdr-gh-nav has to be run by herdr \
                 (as a plugin action or pane), not directly from a shell."
            )
        })?;
        Ok(Self {
            socket_path: PathBuf::from(path),
            next_id: Cell::new(0),
        })
    }

    /// Send one request and return its `result` object.
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let request = json!({
            "id": format!("herdr-gh-nav:{id}"),
            "method": method,
            // `params` is required even when empty; omitting it is an invalid_request.
            "params": params,
        });

        let stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!(
                "could not connect to the herdr API socket at {}. Is the herdr server running?",
                self.socket_path.display()
            )
        })?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;

        let mut writer = &stream;
        writeln!(writer, "{request}").with_context(|| format!("sending {method} to herdr"))?;
        writer.flush()?;

        let mut line = String::new();
        BufReader::new(&stream)
            .read_line(&mut line)
            .with_context(|| format!("reading the herdr response to {method}"))?;
        if line.trim().is_empty() {
            bail!("herdr closed the connection without answering {method}");
        }

        let mut response: Value = serde_json::from_str(&line)
            .with_context(|| format!("herdr sent an unparseable response to {method}"))?;

        if let Some(error) = response.get("error") {
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("no message");
            bail!("herdr rejected {method}: {message} ({code})");
        }

        response
            .get_mut("result")
            .map(Value::take)
            .ok_or_else(|| anyhow!("herdr response to {method} had neither result nor error"))
    }

    /// Call and deserialize the whole `result` object, ignoring its `type` discriminator.
    fn call_into<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let result = self.call(method, params)?;
        serde_json::from_value(result)
            .with_context(|| format!("herdr's {method} response did not match the expected shape"))
    }

    /// Call and deserialize one named field out of the `result` object.
    fn call_field<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
        field: &str,
    ) -> Result<T> {
        let mut result = self.call(method, params)?;
        let value = result
            .get_mut(field)
            .map(Value::take)
            .ok_or_else(|| anyhow!("herdr's {method} response had no `{field}` field"))?;
        serde_json::from_value(value)
            .with_context(|| format!("herdr's {method} `{field}` did not match the expected shape"))
    }
}

impl HerdrPort for SocketHerdr {
    fn snapshot(&self) -> Result<Snapshot> {
        self.call_field("session.snapshot", json!({}), "snapshot")
    }

    fn worktree_list(&self, cwd: &str) -> Result<WorktreeList> {
        self.call_into("worktree.list", json!({ "cwd": cwd }))
    }

    fn worktree_create(&self, req: &WorktreeCreate) -> Result<WorktreeOpened> {
        self.call_into(
            "worktree.create",
            json!({
                "cwd": req.cwd,
                "branch": req.branch,
                "base": req.base,
                "focus": req.focus,
            }),
        )
    }

    fn worktree_open(&self, req: &WorktreeOpen) -> Result<WorktreeOpened> {
        self.call_into(
            "worktree.open",
            json!({
                "cwd": req.cwd,
                "path": req.path,
                "branch": req.branch,
                "focus": req.focus,
            }),
        )
    }

    fn pane_focus(&self, pane_id: &str) -> Result<()> {
        self.call("pane.focus", json!({ "pane_id": pane_id }))?;
        Ok(())
    }

    fn pane_split(&self, req: &PaneSplit) -> Result<Pane> {
        self.call_field(
            "pane.split",
            json!({
                "target_pane_id": req.target_pane_id,
                "direction": req.direction.as_str(),
                "cwd": req.cwd,
                "focus": req.focus,
            }),
            "pane",
        )
    }

    fn pane_move(&self, pane_id: &str, dest: &PaneDestination, focus: bool) -> Result<()> {
        let destination = match dest {
            PaneDestination::Tab {
                tab_id,
                split,
                target_pane_id,
            } => json!({
                "type": "tab",
                "tab_id": tab_id,
                "split": split.as_str(),
                "target_pane_id": target_pane_id,
            }),
            PaneDestination::NewTab { workspace_id } => json!({
                "type": "new_tab",
                "workspace_id": workspace_id,
            }),
        };
        self.call(
            "pane.move",
            json!({ "pane_id": pane_id, "destination": destination, "focus": focus }),
        )?;
        Ok(())
    }

    fn workspace_focus(&self, workspace_id: &str) -> Result<()> {
        self.call("workspace.focus", json!({ "workspace_id": workspace_id }))?;
        Ok(())
    }

    fn tab_focus(&self, tab_id: &str) -> Result<()> {
        self.call("tab.focus", json!({ "tab_id": tab_id }))?;
        Ok(())
    }

    fn plugin_pane_open(&self, req: &PluginPaneOpen) -> Result<Pane> {
        let mut result = self.call(
            "plugin.pane.open",
            json!({
                "plugin_id": req.plugin_id,
                "entrypoint": req.entrypoint,
                "placement": req.placement,
                "cwd": req.cwd,
                "focus": req.focus,
            }),
        )?;
        let pane = result
            .pointer_mut("/plugin_pane/pane")
            .map(Value::take)
            .ok_or_else(|| anyhow!("herdr's plugin.pane.open response had no pane"))?;
        Ok(serde_json::from_value(pane)?)
    }

    fn plugin_pane_focus(&self, pane_id: &str) -> Result<()> {
        self.call("plugin.pane.focus", json!({ "pane_id": pane_id }))?;
        Ok(())
    }
}
