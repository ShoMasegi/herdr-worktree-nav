//! herdr-gh-nav — navigate herdr panes by repo and worktree, and turn branches into
//! worktree panes.
//!
//! The crate is split so that the interesting decisions are testable without herdr or a
//! repository present:
//!
//! - [`port`] declares what the plugin needs from the outside world.
//! - [`adapter`] implements those ports against the herdr socket, `git`, and `gh`.
//! - [`domain`] is pure: it turns port data into the model the UI shows, decides what a
//!   chosen branch means, and plans the herdr calls a chosen destination requires.
//! - [`ui`] renders and handles keys.

pub mod adapter;
pub mod domain;
pub mod port;
pub mod ui;

/// Must match `id` in `herdr-plugin.toml`; herdr addresses panes and actions by it.
pub const PLUGIN_ID: &str = "herdr-gh-nav";
