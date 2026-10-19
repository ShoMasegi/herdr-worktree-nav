//! `HerdrPort` over herdr's Unix socket API.
//!
//! The socket is the transport rather than the `herdr` CLI for one decisive reason:
//! `pane.focus` — jumping to a chosen pane, which is the whole point of the panes view —
//! has no CLI equivalent. The CLI's `pane focus` is directional and `agent focus` rejects
//! panes with no agent. See `docs/adr/0002-socket-transport.md`.
//!
//! Protocol: connect, write one JSON request terminated by a newline, read one JSON
//! response terminated by a newline. The server closes the connection after replying, so
//! every call gets its own connection.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::port::*;

/// Long enough to cover a `worktree.create` that has to check out a large repository,
/// short enough that a wedged server surfaces as an error rather than a hung picker.
const TIMEOUT: Duration = Duration::from_secs(30);

/// herdr's answer to a request it refused: the error object's two fields, kept apart.
///
/// Kept as a value rather than flattened into a sentence, so that a caller deciding on the
/// refusal reads its `code` with [`refused_with`]: `message` is free text the server writes,
/// and a sentence one `.with_context(..)` away from changing is not something to match on
/// (issue #19).
#[derive(Debug)]
struct HerdrError {
    method: String,
    code: String,
    message: String,
}

impl std::fmt::Display for HerdrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "herdr rejected {}: {} ({})",
            self.method, self.message, self.code
        )
    }
}

impl std::error::Error for HerdrError {}

/// Whether `error` is herdr refusing with `code`, through any context added since.
fn refused_with(error: &anyhow::Error, code: &str) -> bool {
    error
        .downcast_ref::<HerdrError>()
        .is_some_and(|refused| refused.code == code)
}

pub struct SocketHerdr {
    socket_path: PathBuf,
    /// Atomic rather than a `Cell` so the picker can call herdr from its worker thread
    /// while the main thread keeps drawing.
    next_id: AtomicU64,
}

impl SocketHerdr {
    /// Locate the socket the way herdr tells plugins to: `HERDR_SOCKET_PATH`, which herdr
    /// injects into every plugin command.
    pub fn from_env() -> Result<Self> {
        let path = std::env::var_os("HERDR_SOCKET_PATH").ok_or_else(|| {
            anyhow!(
                "HERDR_SOCKET_PATH is not set. herdr-worktree-nav has to be run by herdr \
                 (as a plugin action or pane), not directly from a shell."
            )
        })?;
        Ok(Self {
            socket_path: PathBuf::from(path),
            next_id: AtomicU64::new(0),
        })
    }

    /// Send one request and return its `result` object.
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({
            "id": format!("herdr-worktree-nav:{id}"),
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
            return Err(HerdrError {
                method: method.to_string(),
                code: code.to_string(),
                message: message.to_string(),
            }
            .into());
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

    fn pane_close(&self, pane_id: &str) -> Result<()> {
        match self.call("pane.close", json!({ "pane_id": pane_id })) {
            Ok(_) => Ok(()),
            // A pane that is not there is the state this was asked to bring about. It
            // happens on its own — a pane whose command finishes closes itself — and
            // stopping a removal for it would kill the panes before it and then abandon the
            // checkout because one of them had already gone.
            Err(error) if refused_with(&error, "pane_not_found") => Ok(()),
            Err(error) => Err(error),
        }
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
        let result = self.call(
            "pane.move",
            json!({ "pane_id": pane_id, "destination": destination, "focus": focus }),
        )?;

        // herdr answers some refusals with success and a `changed: false` — moving into a
        // zoomed tab is one. Left unchecked that reads as "done" while the pane never went
        // anywhere, so it is turned into the failure it is.
        let moved = result
            .pointer("/move_result/changed")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if !moved {
            let reason = result
                .pointer("/move_result/reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            bail!("herdr did not move the pane into that tab ({reason})");
        }
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

    fn plugin_pane_open(&self, req: &PluginPaneOpen) -> Result<Option<OpenRefusal>> {
        // No placement or size: the manifest carries them, and herdr honours it when the
        // request leaves them null.
        let result = self.call(
            "plugin.pane.open",
            json!({
                "plugin_id": req.plugin_id,
                "entrypoint": req.entrypoint,
                "cwd": req.cwd,
                "env": req
                    .env
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from(value.clone())))
                    .collect::<serde_json::Map<String, Value>>(),
                "focus": req.focus,
            }),
        );
        match result {
            Ok(_) => Ok(None),
            // herdr allows one popup at a time. Reported as a failed open, but from here it
            // is simply "the picker is already up", which is what the caller asked for.
            Err(error) if error.to_string().contains("popup already open") => {
                Ok(Some(OpenRefusal::PopupAlreadyOpen))
            }
            Err(error) => Err(error),
        }
    }

    fn notify(&self, notification: &Notification) -> Result<()> {
        // The answer carries `shown` and a reason — `disabled`, `no_foreground_client`,
        // `rate_limited`, `busy`. None of them is acted on: a user who has turned
        // notifications off has said so, and routing around that would be overruling them.
        // No `position` either; herdr's configuration decides where its own toasts go.
        self.call(
            "notification.show",
            json!({
                "title": notification.title,
                "body": notification.body,
                "sound": notification.sound.as_str(),
            }),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(code: &str, message: &str) -> anyhow::Error {
        HerdrError {
            method: "pane.close".to_string(),
            code: code.to_string(),
            message: message.to_string(),
        }
        .into()
    }

    #[test]
    fn a_refusal_is_read_by_its_code_and_never_by_its_words() {
        assert!(refused_with(
            &refusal("pane_not_found", "no pane w1:p9"),
            "pane_not_found"
        ));
        // Issue #19: a message is the server's free text, and one that names the code
        // is not a refusal with it.
        assert!(!refused_with(
            &refusal(
                "invalid_params",
                "pane_id must name a pane; expected one of the errors pane_not_found, …"
            ),
            "pane_not_found"
        ));
        assert!(!refused_with(
            &anyhow!("herdr closed the connection without answering pane.close"),
            "pane_not_found"
        ));
    }

    #[test]
    fn context_added_at_the_call_site_does_not_change_what_a_refusal_is() {
        let error = refusal("pane_not_found", "no pane w1:p9").context("closing w1:p9");
        assert!(refused_with(&error, "pane_not_found"));
        assert_eq!(
            format!("{error:#}"),
            "closing w1:p9: herdr rejected pane.close: no pane w1:p9 (pane_not_found)",
            "and the sentence a reader gets is the one it always was"
        );
    }
}
