//! The preferences from the plugin configuration file.
//!
//! The parser stays pure. The adapter finds and reads the file.

use serde::Deserialize;

/// Preferences that belong to this plugin, not to herdr.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub panes: Panes,
}

/// Preferences for the panes view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Panes {
    /// Whether the first frame includes worktrees that contain no pane.
    pub worktree_nav_show_no_panes: bool,
}

impl Default for Panes {
    fn default() -> Self {
        Self {
            // Preserve the view from releases before this toggle existed.
            worktree_nav_show_no_panes: true,
        }
    }
}

/// Read plugin settings from TOML.
pub fn parse(config_toml: &str) -> Result<Settings, toml::de::Error> {
    toml::from_str(config_toml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_keeps_worktrees_without_panes_visible() {
        assert!(parse("").unwrap().panes.worktree_nav_show_no_panes);
    }

    #[test]
    fn the_panes_setting_can_hide_worktrees_without_panes() {
        let settings = parse("[panes]\nworktree_nav_show_no_panes = false\n").unwrap();
        assert!(!settings.panes.worktree_nav_show_no_panes);
    }

    #[test]
    fn an_unknown_setting_is_an_error_instead_of_a_silent_typo() {
        assert!(parse("[panes]\nworktree_nav_show_no_pane = true\n").is_err());
    }
}
