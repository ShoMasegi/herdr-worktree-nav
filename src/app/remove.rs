//! The removal, in the process that outlives the picker.
//!
//! Started by [`crate::port::RemovalPort`] and never by herdr, which is what makes this the
//! one mode that has to report itself: nothing is holding a screen for it, and its stderr
//! goes nowhere `herdr plugin log list` collects. See
//! `docs/adr/0014-removing-outlives-the-picker.md`.

use std::io::Write;

use anyhow::Result;

use crate::domain::removal;
use crate::port::{GitPort, HerdrPort, RemovalOutcome};

/// Remove one checkout, tell the user, and tell whoever started this if they are still
/// listening.
///
/// git declining is an outcome rather than a failure — a checkout with uncommitted work is
/// exactly what it is meant to protect — so this returns `Ok` either way and the answer
/// travels in the report instead of in an exit code.
pub fn run(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    repo_root: &str,
    checkout_path: &str,
    label: &str,
) -> Result<()> {
    let outcome = match git.remove_worktree(repo_root, checkout_path) {
        Ok(()) => RemovalOutcome::Removed,
        Err(error) => RemovalOutcome::Refused(format!("{error:#}")),
    };

    // The toast first, and deliberately. It is the report that always happens, and the
    // write below is the one thing here that can end this process early: the picker may
    // have closed, and a pipe with no reader left is what that looks like from this side.
    // herdr declining to show it is herdr's answer to give, so it is not retried or
    // reported anywhere else.
    let _ = herdr.notify(&removal::notification(label, checkout_path, &outcome));

    let _ = writeln!(std::io::stdout(), "{}", removal::report_line(&outcome));
    Ok(())
}
