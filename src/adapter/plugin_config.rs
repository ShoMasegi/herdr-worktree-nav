//! Read this plugin's own configuration file.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::domain::settings::{self, Settings};
use crate::PLUGIN_ID;

const CONFIG_FILE: &str = "config.toml";
/// How the prompt line names this file, so it is not mixed up with herdr's own `config.toml`.
const PROMPT_FILE: &str = "plugin config.toml";

/// What the picker should use, and what the prompt line should say if the file could not.
pub struct Loaded {
    pub settings: Settings,
    pub complaint: Option<String>,
    /// The file that supplied the settings. `None` means that no file exists.
    pub path: Option<PathBuf>,
}

/// Load the plugin settings. A missing file uses the documented defaults. A file that
/// cannot be read or parsed does too, and names itself so the prompt line can say so —
/// a typo must not take the picker down.
pub fn load() -> Loaded {
    let Some(path) = config_path() else {
        return Loaded::missing();
    };
    load_from(&path)
}

impl Loaded {
    fn missing() -> Self {
        Self {
            settings: Settings::default(),
            complaint: None,
            path: None,
        }
    }

    fn ok(path: &Path, settings: Settings) -> Self {
        Self {
            settings,
            complaint: None,
            path: Some(path.to_path_buf()),
        }
    }

    fn unreadable(path: &Path, error: impl std::fmt::Display) -> Self {
        Self {
            settings: Settings::default(),
            complaint: Some(format!("could not read {PROMPT_FILE}: {error}")),
            path: Some(path.to_path_buf()),
        }
    }

    fn invalid(path: &Path, contents: &str, error: toml::de::Error) -> Self {
        // The prompt line is one row. A short file label, then the reason without the
        // caret drawing — a cut must not leave only a path. A syntax error has no key
        // in the reason, so the line number is what locates it.
        Self {
            settings: Settings::default(),
            complaint: Some(format_invalid(contents, error)),
            path: Some(path.to_path_buf()),
        }
    }
}

fn format_invalid(contents: &str, error: toml::de::Error) -> String {
    let reason = error.message();
    // An unknown key already names itself. A syntax error does not, so the line
    // number is what locates it.
    if reason.starts_with("unknown field") {
        return format!("{PROMPT_FILE}: {reason}");
    }
    match error.span() {
        Some(span) if contents.is_char_boundary(span.start) => {
            let line = 1 + contents[..span.start]
                .bytes()
                .filter(|&b| b == b'\n')
                .count();
            format!("{PROMPT_FILE}: line {line}: {reason}")
        }
        _ => format!("{PROMPT_FILE}: {reason}"),
    }
}

/// Read one resolved path. A missing file uses the documented defaults.
fn load_from(path: &Path) -> Loaded {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Loaded::missing();
        }
        Err(error) => return Loaded::unreadable(path, error),
    };
    match settings::parse(&contents) {
        Ok(settings) => Loaded::ok(path, settings),
        Err(error) => Loaded::invalid(path, &contents, error),
    }
}

/// Find the directory that herdr reserves for this plugin.
fn config_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join(CONFIG_FILE));
    }

    let roots = [
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        dirs::home_dir().map(|home| home.join(".config")),
    ];
    roots
        .into_iter()
        .flatten()
        .map(|root| {
            root.join("herdr")
                .join("plugins")
                .join("config")
                .join(PLUGIN_ID)
                .join(CONFIG_FILE)
        })
        .find(|path| path.is_file())
}

#[cfg(test)]
pub(crate) fn complaint_for(contents: &str) -> String {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(contents.as_bytes()).unwrap();
    load_from(file.path())
        .complaint
        .unwrap_or_else(|| panic!("expected a complaint for {contents:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_missing_file_uses_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_from(&dir.path().join(CONFIG_FILE));
        assert_eq!(loaded.settings, Settings::default());
        assert!(loaded.complaint.is_none());
        assert!(loaded.path.is_none());
    }

    #[test]
    fn the_file_controls_the_initial_panes_view() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"[panes]\nworktree_nav_show_no_panes = false\n")
            .unwrap();
        let loaded = load_from(file.path());
        assert!(!loaded.settings.panes.worktree_nav_show_no_panes);
        assert!(loaded.complaint.is_none());
        assert_eq!(loaded.path.as_deref(), Some(file.path()));
    }

    #[test]
    fn an_invalid_file_keeps_the_defaults_and_leads_with_the_reason() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"[panes]\nunknown = true\n").unwrap();
        let loaded = load_from(file.path());
        assert_eq!(loaded.settings, Settings::default());
        let complaint = loaded.complaint.expect("malformed file should complain");
        assert!(
            complaint.starts_with("plugin config.toml: unknown field"),
            "{complaint}"
        );
        assert!(
            !complaint.contains("TOML parse error"),
            "caret drawing would push the reason off the prompt line: {complaint}"
        );
        assert!(
            !complaint.contains(file.path().to_str().unwrap()),
            "the full path would hide the reason: {complaint}"
        );
    }

    #[test]
    fn a_wrong_table_keeps_the_defaults_and_leads_with_the_reason() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"[pane]\nworktree_nav_show_no_panes = false\n")
            .unwrap();
        let loaded = load_from(file.path());
        assert!(loaded.settings.panes.worktree_nav_show_no_panes);
        let complaint = loaded.complaint.expect("wrong table should complain");
        assert!(
            complaint.starts_with("plugin config.toml: unknown field `pane`"),
            "{complaint}"
        );
        assert!(complaint.contains("expected `panes`"), "{complaint}");
    }

    #[test]
    fn a_near_miss_key_names_the_wrong_key_and_the_right_one() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"[panes]\nworktree_nav_show_no_pane = false\n")
            .unwrap();
        let loaded = load_from(file.path());
        let complaint = loaded.complaint.expect("a near-miss key should complain");
        assert!(
            complaint.starts_with("plugin config.toml: unknown field `worktree_nav_show_no_pane`"),
            "{complaint}"
        );
        assert!(
            complaint.contains("expected `worktree_nav_show_no_panes`"),
            "{complaint}"
        );
    }

    #[test]
    fn an_unreadable_path_keeps_the_defaults_and_says_it_could_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_from(dir.path());
        assert_eq!(loaded.settings, Settings::default());
        let complaint = loaded.complaint.expect("a directory is not a file");
        assert!(
            complaint.starts_with("could not read plugin config.toml:"),
            "{complaint}"
        );
    }

    #[test]
    fn a_syntax_error_names_the_line() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(
            b"[panes]\nworktree_nav_show_no_panes = true\nworktree_nav_show_no_panes = false\n",
        )
        .unwrap();
        let loaded = load_from(file.path());
        let complaint = loaded.complaint.expect("a duplicate key should complain");
        assert!(
            complaint.starts_with("plugin config.toml: line 3: duplicate key"),
            "{complaint}"
        );
        assert!(
            !complaint.contains("TOML parse error"),
            "caret drawing would push the reason off the prompt line: {complaint}"
        );
    }
}
