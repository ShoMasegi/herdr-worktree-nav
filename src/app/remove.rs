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
    /// hand stays three arguments.
    pub panes_closed: usize,
    /// Whether `git branch -d` follows the removal: the literal `delete-branch` anywhere
    /// after the branch, or nothing. A word there that is neither a count nor the flag is an
    /// error rather than a guess — a wire this side does not understand is not something to
    /// decide a branch's fate on either way.
    pub delete_branch: bool,
}

impl Args {
    pub fn read(args: &mut impl Iterator<Item = String>) -> Result<Self> {
        let (Some(repo_root), Some(checkout_path), Some(label)) =
            (args.next(), args.next(), args.next())
        else {
            anyhow::bail!("`remove` needs a repository root, a checkout path, and a branch name")
        };
        // In either order, so `delete-branch` typed with no count before it is read as the
        // flag rather than as a count that would not parse.
        let mut panes_closed = 0;
        let mut delete_branch = false;
        for token in args {
            match token.parse::<usize>() {
                Ok(count) => panes_closed = count,
                Err(_) if token == "delete-branch" => delete_branch = true,
                Err(_) => anyhow::bail!(
                    "`remove` does not understand `{token}`: after the branch come a pane \
                     count, `delete-branch`, or nothing"
                ),
            }
        }
        Ok(Self {
            repo_root,
            checkout_path,
            label,
            panes_closed,
            delete_branch,
        })
    }
}

/// Remove one checkout, tell the user, and tell whoever started this if they are still
/// listening. `delete_branch` asks for `git branch -d` on `label` once the checkout has
/// gone, and only then: a refused removal never reaches the branch.
///
/// `panes_closed` is what the picker stopped to get here. It is passed in rather than
/// worked out because by the time this runs the panes are already gone: the grouping this
/// process could rebuild for itself would be a grouping with nothing in it.
///
/// git declining is an outcome rather than a failure, so this returns `Ok` either way and
/// the answer travels in the report instead of in an exit code.
pub fn run(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    repo_root: &str,
    checkout_path: &str,
    label: &str,
    panes_closed: usize,
    delete_branch: bool,
) -> Result<()> {
    let outcome = outcome(git, repo_root, checkout_path, label, delete_branch);

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

/// What became of the checkout and, when asked, its branch. Apart from `run` so a test can
/// read it: the line `run` writes goes down stdout, where a test cannot.
fn outcome(
    git: &dyn GitPort,
    repo_root: &str,
    checkout_path: &str,
    label: &str,
    delete_branch: bool,
) -> RemovalOutcome {
    if let Err(error) = git.remove_worktree(repo_root, checkout_path) {
        return RemovalOutcome::Refused(format!("{error:#}"));
    }
    if !delete_branch {
        return RemovalOutcome::Removed;
    }
    match git.delete_branch(repo_root, label) {
        Ok(()) => RemovalOutcome::Removed,
        Err(error) => RemovalOutcome::BranchKept(format!("{error:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{
        Notification, Pane, PaneDestination, PaneSplit, PluginPaneOpen, RefWalk, RepoIdentity,
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

    /// A removal refused: the words `removing_a_worktree_with_uncommitted_work_refuses_and_says_why`
    /// in `tests/git_adapter.rs` asserts on.
    const DIRTY: &str = "fatal: '/wt/feat-login' contains modified or untracked files";
    /// A `-d` declined: the words `a_branch_git_does_not_call_merged_is_kept_and_git_says_why`
    /// in `tests/git_adapter.rs` asserts on.
    const NOT_MERGED: &str =
        "error: the branch 'feat/login' is not fully merged (`git branch -d feat/login`)";

    /// A git that answers the two calls `run` can make as told, and keeps which ones it got.
    struct Git {
        remove: Result<(), &'static str>,
        /// `None` is a `-d` the test says must never be asked for.
        delete: Option<Result<(), &'static str>>,
        asked: Mutex<Vec<String>>,
    }

    impl Git {
        fn new(remove: Result<(), &'static str>, delete: Option<Result<(), &'static str>>) -> Self {
            Self {
                remove,
                delete,
                asked: Mutex::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl GitPort for Git {
        fn remove_worktree(&self, _repo_root: &str, checkout_path: &str) -> Result<()> {
            self.asked
                .lock()
                .unwrap()
                .push(format!("remove {checkout_path}"));
            self.remove.map_err(|said| anyhow::anyhow!("{said}"))
        }
        fn delete_branch(&self, _repo_root: &str, branch: &str) -> Result<()> {
            self.asked.lock().unwrap().push(format!("delete {branch}"));
            let answer = self.delete.expect("no `-d` was to be asked for");
            answer.map_err(|said| anyhow::anyhow!("{said}"))
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
        fn local_refs(&self, _repo_root: &str) -> Result<RefWalk> {
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

    /// The toast `run` shows for this git, with no panes closed.
    fn toast(git: &Git, delete_branch: bool) -> Notification {
        let herdr = Shown::default();
        run(
            &herdr,
            git,
            "/src/app",
            "/wt/feat-login",
            "feat/login",
            0,
            delete_branch,
        )
        .unwrap();
        let shown = herdr.0.lock().unwrap();
        shown[0].clone()
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
        assert!(!read.delete_branch, "and asked for no branch to go");

        // A fourth argument that is neither a count nor the flag is refused: read as a
        // count of none it would have swallowed `delete-branch` typed in its place. A near
        // miss of the flag is refused the same way, not read as the flag.
        for odd in ["later", "delete-branches", "delete_branch"] {
            assert!(
                Args::read(
                    &mut ["/src/app", "/wt/x", "fix/crash", odd]
                        .iter()
                        .map(|a| a.to_string())
                )
                .is_err(),
                "{odd} should be refused"
            );
        }

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
            &Git::new(Err(DIRTY), None),
            "/src/app",
            "/wt/feat-login",
            "feat/login",
            2,
            false,
        )
        .unwrap();

        let shown = herdr.0.lock().unwrap();
        let body = shown[0].body.as_deref().expect("a refusal says why");
        assert!(
            body.ends_with("— its 2 panes were closed first"),
            "got {body}"
        );
    }

    #[test]
    fn with_the_branch_asked_for_a_d_git_takes_is_reported_as_removed() {
        // The checkout first, then the branch, and one plain `removed` for the pair.
        let git = Git::new(Ok(()), Some(Ok(())));
        let outcome = outcome(&git, "/src/app", "/wt/feat-login", "feat/login", true);
        assert_eq!(git.asked(), ["remove /wt/feat-login", "delete feat/login"]);
        assert_eq!(removal::report_line(&outcome), "removed");
        assert_eq!(toast(&git, true).title, "removed feat/login");
    }

    #[test]
    fn a_d_git_declines_keeps_the_branch_and_says_so_in_gits_words() {
        // The checkout is gone by then, so this is neither a refusal nor a plain removal.
        let git = Git::new(Ok(()), Some(Err(NOT_MERGED)));
        let outcome = outcome(&git, "/src/app", "/wt/feat-login", "feat/login", true);
        assert_eq!(outcome, RemovalOutcome::BranchKept(NOT_MERGED.to_string()));
        assert!(
            removal::report_line(&outcome).starts_with("kept "),
            "got {}",
            removal::report_line(&outcome)
        );

        let toast = toast(&git, true);
        assert_eq!(toast.title, "removed feat/login, branch kept");
        let body = toast.body.as_deref().expect("says why the branch stayed");
        assert!(body.ends_with(NOT_MERGED), "git's words last: {body}");
        assert_eq!(toast.sound, crate::port::NotificationSound::None);
    }

    #[test]
    fn without_the_flag_the_branch_is_never_asked_about() {
        // `Shift-D`'s removal: the fake's `-d` panics, so reaching it fails this.
        let git = Git::new(Ok(()), None);
        let outcome = outcome(&git, "/src/app", "/wt/feat-login", "feat/login", false);
        assert_eq!(outcome, RemovalOutcome::Removed);
        assert_eq!(toast(&git, false).title, "removed feat/login");
        assert_eq!(
            git.asked(),
            // Twice: `outcome` for the report line and `run` for the toast, on one fake.
            ["remove /wt/feat-login", "remove /wt/feat-login"]
        );
    }

    #[test]
    fn a_refused_removal_never_reaches_the_branch() {
        // The checkout is still there with work in it, and its branch has to stay with it.
        let git = Git::new(Err(DIRTY), None);
        let outcome = outcome(&git, "/src/app", "/wt/feat-login", "feat/login", true);
        assert_eq!(outcome, RemovalOutcome::Refused(DIRTY.to_string()));
        assert_eq!(toast(&git, true).title, "could not remove feat/login");
        assert_eq!(
            git.asked(),
            // Twice: `outcome` for the report line and `run` for the toast, on one fake.
            ["remove /wt/feat-login", "remove /wt/feat-login"]
        );
    }

    #[test]
    fn the_flag_is_read_with_or_without_a_pane_count_before_it() {
        // `main`'s usage line offers `[panes-closed] [delete-branch]` as two optional words,
        // so the flag alone has to mean the flag; either order is the parser's own choice.
        let read = |words: &[&str]| {
            Args::read(&mut words.iter().map(|a| a.to_string())).expect("its own usage line")
        };
        let flag_alone = read(&["/src/app", "/wt/x", "fix/crash", "delete-branch"]);
        assert_eq!(
            (flag_alone.panes_closed, flag_alone.delete_branch),
            (0, true)
        );
        let count_first = read(&["/src/app", "/wt/x", "fix/crash", "2", "delete-branch"]);
        assert_eq!(
            (count_first.panes_closed, count_first.delete_branch),
            (2, true)
        );
        let flag_first = read(&["/src/app", "/wt/x", "fix/crash", "delete-branch", "2"]);
        assert_eq!(
            (flag_first.panes_closed, flag_first.delete_branch),
            (2, true)
        );
    }
}
