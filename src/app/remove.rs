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

/// What the `remove` mode was told to do, read off the command line.
///
/// The other half of this wire format is `adapter::detached::arguments`, in another module
/// and, by the time it is read, in another process. Neither half can see the other, so the
/// only thing keeping them in step is a test that runs one into the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub repo_root: String,
    pub checkout_path: String,
    pub label: String,
    /// How many panes the picker closed to get here. Absent means none, so running this by
    /// hand stays three arguments; unreadable means none too, which understates rather than
    /// invents and only ever costs the report one clause.
    pub panes_closed: usize,
}

impl Args {
    pub fn read(args: &mut impl Iterator<Item = String>) -> Result<Self> {
        let (Some(repo_root), Some(checkout_path), Some(label)) =
            (args.next(), args.next(), args.next())
        else {
            anyhow::bail!("`remove` needs a repository root, a checkout path, and a branch name")
        };
        Ok(Self {
            repo_root,
            checkout_path,
            label,
            panes_closed: args.next().and_then(|n| n.parse().ok()).unwrap_or(0),
        })
    }
}

/// Remove one checkout, tell the user, and tell whoever started this if they are still
/// listening.
///
/// `panes_closed` is what the picker stopped to get here. It is passed in rather than
/// worked out because by the time this runs the panes are already gone: the grouping this
/// process could rebuild for itself would be a grouping with nothing in it.
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
    panes_closed: usize,
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
    let _ = herdr.notify(&removal::notification(
        label,
        checkout_path,
        &outcome,
        panes_closed,
    ));

    let _ = writeln!(std::io::stdout(), "{}", removal::report_line(&outcome));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{
        GitRef, Notification, Pane, PaneDestination, PaneSplit, PluginPaneOpen, RepoIdentity,
        Snapshot, WorktreeCreate, WorktreeList, WorktreeOpen, WorktreeOpened,
    };
    use std::sync::Mutex;

    /// Keeps the toast it was asked to show, which is the only report that reaches somebody
    /// who has closed the picker.
    #[derive(Default)]
    struct Shown(Mutex<Vec<Notification>>);

    impl HerdrPort for Shown {
        fn notify(&self, notification: &Notification) -> Result<()> {
            self.0.lock().unwrap().push(notification.clone());
            Ok(())
        }
        fn snapshot(&self) -> Result<Snapshot> {
            unreachable!()
        }
        fn worktree_list(&self, _cwd: &str) -> Result<WorktreeList> {
            unreachable!()
        }
        fn worktree_create(&self, _req: &WorktreeCreate) -> Result<WorktreeOpened> {
            unreachable!()
        }
        fn worktree_open(&self, _req: &WorktreeOpen) -> Result<WorktreeOpened> {
            unreachable!()
        }
        fn pane_focus(&self, _pane_id: &str) -> Result<()> {
            unreachable!()
        }
        fn pane_close(&self, _pane_id: &str) -> Result<()> {
            unreachable!()
        }
        fn pane_split(&self, _req: &PaneSplit) -> Result<Pane> {
            unreachable!()
        }
        fn pane_move(&self, _p: &str, _d: &PaneDestination, _f: bool) -> Result<()> {
            unreachable!()
        }
        fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
            unreachable!()
        }
        fn tab_focus(&self, _tab_id: &str) -> Result<()> {
            unreachable!()
        }
        fn plugin_pane_open(
            &self,
            _req: &PluginPaneOpen,
        ) -> Result<Option<crate::port::OpenRefusal>> {
            unreachable!()
        }
    }

    /// Refuses the removal the way git refuses a checkout with work in it.
    struct Refuses;

    impl GitPort for Refuses {
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            anyhow::bail!("fatal: '/wt/feat-login' contains modified or untracked files")
        }
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            unreachable!()
        }
        fn identify(&self, _cwd: &str) -> Result<Option<RepoIdentity>> {
            unreachable!()
        }
        fn github_slug(&self, _repo_root: &str) -> Result<Option<crate::port::Slug>> {
            unreachable!()
        }
        fn local_refs(&self, _repo_root: &str) -> Result<Vec<GitRef>> {
            unreachable!()
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!()
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!()
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!()
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!()
        }
    }

    #[test]
    fn running_it_by_hand_stays_three_arguments() {
        // The count is the picker's to supply; a person typing this has closed nothing.
        let read = Args::read(
            &mut ["/src/app", "/wt/x", "fix/crash"]
                .iter()
                .map(|a| a.to_string()),
        )
        .unwrap();
        assert_eq!(read.panes_closed, 0);

        // And a fourth argument that is not a number understates rather than invents. It
        // costs the report one clause and never changes what is removed.
        let odd = Args::read(
            &mut ["/src/app", "/wt/x", "fix/crash", "later"]
                .iter()
                .map(|a| a.to_string()),
        )
        .unwrap();
        assert_eq!(odd.panes_closed, 0);

        assert!(Args::read(&mut ["/src/app", "/wt/x"].iter().map(|a| a.to_string())).is_err());
    }

    #[test]
    fn the_panes_this_closed_reach_the_one_report_a_departed_user_gets() {
        // The count travels from the picker through argv into this process for one purpose,
        // and the toast is where it has to arrive: whoever closed the picker has no other
        // channel, and a refusal that does not mention the panes reads as "nothing
        // happened" over an emptied tab.
        let herdr = Shown::default();
        run(
            &herdr,
            &Refuses,
            "/src/app",
            "/wt/feat-login",
            "feat/login",
            2,
        )
        .unwrap();

        let shown = herdr.0.lock().unwrap();
        let body = shown[0].body.as_deref().expect("a refusal says why");
        assert!(
            body.ends_with("— its 2 panes were closed first"),
            "got {body}"
        );
    }
}
