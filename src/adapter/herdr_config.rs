//! Reading herdr's own `config.toml`, so the pickers can borrow its accent colour and
//! status glyphs.
//!
//! This is the only file outside herdr's plugin directories that the plugin touches, and it
//! only ever reads. Everything about interpreting the contents lives in
//! `crate::domain::chrome`; this module just finds the file.

use std::path::PathBuf;

use crate::domain::chrome::{self, Chrome};

const CONFIG_FILE: &str = "config.toml";

/// Load herdr's chrome, falling back to herdr's own defaults when the config cannot be
/// found or read. A missing config is the normal state for a fresh install, not an error.
pub fn load() -> Chrome {
    config_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|contents| chrome::parse(&contents))
        .unwrap_or_default()
}

/// Where herdr keeps its configuration.
///
/// The first candidate is derived from `HERDR_PLUGIN_CONFIG_DIR`, which herdr injects and
/// which sits at `<herdr config>/plugins/config/<plugin id>` — following herdr's own answer
/// beats guessing at it. The rest are the conventional locations, for a plugin run outside
/// that environment (`herdr-worktree-nav dump` from a plain shell, for instance).
fn config_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        // <herdr config>/plugins/config/<plugin id> -> <herdr config>
        if let Some(root) = PathBuf::from(dir)
            .ancestors()
            .nth(3)
            .map(PathBuf::from)
            .filter(|root| root.join(CONFIG_FILE).is_file())
        {
            return Some(root.join(CONFIG_FILE));
        }
    }

    let candidates = [
        std::env::var_os("XDG_CONFIG_HOME").map(|dir| PathBuf::from(dir).join("herdr")),
        dirs::home_dir().map(|home| home.join(".config").join("herdr")),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(|dir| dir.join(CONFIG_FILE))
        .find(|path| path.is_file())
}
