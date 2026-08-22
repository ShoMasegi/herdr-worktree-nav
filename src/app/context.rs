//! What herdr tells a plugin command about where it was invoked from.
//!
//! An action runs with the plugin directory as its working directory, so the only way to
//! know which repository the user was looking at is `HERDR_PLUGIN_CONTEXT_JSON`. The pane it
//! then opens does not get that context, so the action forwards what matters as environment.

use serde::Deserialize;

/// The environment variables the action sets on the pane it opens. herdr's own
/// `HERDR_PANE_ID` in a pane process is that pane's id, not the one it was summoned from.
pub const FROM_PANE: &str = "GH_NAV_FROM_PANE";
pub const REPO_ROOT: &str = "GH_NAV_REPO_ROOT";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Context {
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    #[serde(default)]
    pub focused_pane_cwd: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub tab_id: Option<String>,
    /// Present when the invoking workspace is one herdr made for a worktree, which saves
    /// running git to find out which repository it is.
    #[serde(default)]
    pub worktree: Option<ContextWorktree>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContextWorktree {
    pub repo_root: String,
    #[serde(default)]
    pub checkout_path: String,
}

impl Context {
    /// Read the context herdr injected. A missing or malformed value is not an error: the
    /// picker still works, it just cannot preselect anything.
    pub fn from_env() -> Self {
        std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// The directory the picker should treat as "where I was summoned from".
    pub fn cwd(&self) -> Option<&str> {
        self.worktree
            .as_ref()
            .map(|worktree| worktree.checkout_path.as_str())
            .filter(|path| !path.is_empty())
            .or(self.focused_pane_cwd.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_shape_herdr_actually_sends() {
        let context: Context = serde_json::from_str(
            r#"{
                "focused_pane_id": "w1:p1",
                "focused_pane_cwd": "/src/app",
                "workspace_id": "w1",
                "tab_id": "w1:t1",
                "invocation_source": "keybinding",
                "worktree": {
                    "repo_key": "/src/app/.git",
                    "repo_name": "app",
                    "repo_root": "/src/app",
                    "checkout_path": "/wt/feat-login",
                    "is_linked_worktree": true
                }
            }"#,
        )
        .unwrap();
        assert_eq!(context.focused_pane_id.as_deref(), Some("w1:p1"));
        assert_eq!(
            context.cwd(),
            Some("/wt/feat-login"),
            "the checkout wins over the pane cwd"
        );
    }

    #[test]
    fn falls_back_to_the_pane_cwd_when_the_workspace_is_not_a_worktree() {
        let context: Context =
            serde_json::from_str(r#"{"focused_pane_id":"w1:p1","focused_pane_cwd":"/src/app"}"#)
                .unwrap();
        assert_eq!(context.cwd(), Some("/src/app"));
    }

    #[test]
    fn an_absent_or_broken_context_is_not_an_error() {
        assert!(Context::default().cwd().is_none());
        assert!(serde_json::from_str::<Context>("{}").is_ok());
    }
}
