//! What the tests in this layer drive their ports with, and wait on.
//!
//! Here rather than in one module's `mod tests` because the ordering rule these exist to
//! pin — panes close, then the removal starts — spans two ports. Both write into one
//! `Recorder` log, so a test reads a single interleaving rather than two sequences it has
//! to merge by eye, and `record` is private so that nothing else can put a line into it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::{anyhow, Result};

use crate::port::{
    Notification, OpenRefusal, Pane, PaneDestination, PaneSplit, PluginPaneOpen, RemovalOutcome,
    RemovalPort, RunningRemoval, Snapshot, WorktreeCreate, WorktreeOpen, WorktreeOpened,
};

/// Spin until `ready`, or fail the test with `what`.
///
/// The work these tests drive runs on threads of its own, so there is nothing to join on and
/// nothing to block for. `what` is what makes the difference between "this hung" and "this
/// machine is loaded" readable when the budget runs out. Here rather than in each module so
/// that the budget is one number for the whole layer.
pub fn until(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..2000 {
        if ready() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("{what}");
}

/// A `HerdrPort` that keeps what it was asked to do, in order, and can be told to refuse one
/// pane.
///
/// A method this does not need is `unreachable!()`, and that is a rule rather than an
/// oversight: the tests that share this rely on an unexpected call failing. A module that
/// needs one of them adds a fake of its own — filling one in with an *answer* would quietly
/// weaken every test already using it, and none of them would fail to say so.
///
/// `snapshot` is the exception, because it refuses rather than answers: a caller that
/// reaches it still fails, in a shape a test can assert on instead of a panic, which is what
/// makes "the panes closed and the list could not be read again" reachable at all. The `Ok`
/// half of that arm needs a herdr that answers — [`app::panes`]'s `Closing`.
#[derive(Default)]
pub struct Recorder {
    did: Mutex<Vec<String>>,
    refuse: Option<String>,
}

impl Recorder {
    pub fn refusing(pane_id: &str) -> Self {
        Self {
            refuse: Some(pane_id.to_string()),
            ..Self::default()
        }
    }

    fn record(&self, what: String) {
        self.did.lock().unwrap().push(what);
    }

    pub fn did(&self) -> Vec<String> {
        self.did.lock().unwrap().clone()
    }
}

impl FakeHerdr for Recorder {
    fn pane_close(&self, pane_id: &str) -> Result<()> {
        if self.refuse.as_deref() == Some(pane_id) {
            return Err(anyhow!(
                "herdr rejected pane.close: no such pane (not_found)"
            ));
        }
        self.record(format!("close {pane_id}"));
        Ok(())
    }

    /// A herdr that will not describe itself. See the note on the struct.
    fn snapshot(&self) -> Result<Snapshot> {
        Err(anyhow::anyhow!("herdr is not answering"))
    }
}
fake_herdr!(Recorder);

/// A `RemovalPort` that starts nothing; it only says that it was asked, into the same log
/// the pane closes go to. That interleaving is the only place ADR 0010's ordering shows up
/// as something a test can read.
pub struct Started<'a>(pub &'a Recorder);

impl RemovalPort for Started<'_> {
    fn start(
        &self,
        _repo_root: &str,
        checkout_path: &str,
        _label: &str,
        panes_closed: usize,
        delete_branch: bool,
    ) -> Result<Box<dyn RunningRemoval>> {
        // A suffix rather than a word on every line, so a test that never asks for the
        // branch reads the line it always read.
        let branch = if delete_branch {
            " deleting the branch"
        } else {
            ""
        };
        self.0.record(format!(
            "start {checkout_path} after {panes_closed}{branch}"
        ));
        Ok(Box::new(Done))
    }
}

/// A removal that has already finished by the time anyone waits on it.
struct Done;

impl RunningRemoval for Done {
    fn wait(self: Box<Self>) -> Result<RemovalOutcome> {
        Ok(RemovalOutcome::Removed)
    }
}

/// A `RemovalPort` that refuses the first start it is asked for and records every one after,
/// the way [`Started`] does. One refusal among many is what a sweep has to carry on past.
pub struct RefusesFirst<'a> {
    rest: Started<'a>,
    asked: AtomicBool,
}

impl<'a> RefusesFirst<'a> {
    pub fn new(recorder: &'a Recorder) -> Self {
        Self {
            rest: Started(recorder),
            asked: AtomicBool::new(false),
        }
    }
}

impl RemovalPort for RefusesFirst<'_> {
    fn start(
        &self,
        repo_root: &str,
        checkout_path: &str,
        label: &str,
        panes_closed: usize,
        delete_branch: bool,
    ) -> Result<Box<dyn RunningRemoval>> {
        if !self.asked.swap(true, Ordering::SeqCst) {
            return Err(anyhow!("could not spawn: no such file or directory"));
        }
        self.rest
            .start(repo_root, checkout_path, label, panes_closed, delete_branch)
    }
}

/// A `RemovalPort` whose every removal has already finished, with the outcome given.
pub struct Reports(pub RemovalOutcome);

impl RemovalPort for Reports {
    fn start(
        &self,
        _repo_root: &str,
        _checkout_path: &str,
        _label: &str,
        _panes_closed: usize,
        _delete_branch: bool,
    ) -> Result<Box<dyn RunningRemoval>> {
        Ok(Box::new(Reported(self.0.clone())))
    }
}

struct Reported(RemovalOutcome);

impl RunningRemoval for Reported {
    fn wait(self: Box<Self>) -> Result<RemovalOutcome> {
        Ok(self.0)
    }
}

/// A `RemovalPort` whose every removal ends without a word this side can read: the last arm of
/// [`adapter::detached::Detached::wait`], where nothing a report could be parsed out of came
/// back. The removal may well have happened, and nothing here knows which.
pub struct Lost;

impl RemovalPort for Lost {
    fn start(
        &self,
        _repo_root: &str,
        _checkout_path: &str,
        label: &str,
        _panes_closed: usize,
        _delete_branch: bool,
    ) -> Result<Box<dyn RunningRemoval>> {
        Ok(Box::new(Ended(label.to_string())))
    }
}

/// Worded as the adapter words it: what the picker does turns on there being no outcome
/// rather than on the sentence, but a test reading the prompt line reads the sentence.
struct Ended(String);

impl RunningRemoval for Ended {
    fn wait(self: Box<Self>) -> Result<RemovalOutcome> {
        Err(anyhow!(
            "the removal of {} ended without saying what happened",
            self.0
        ))
    }
}

/// A `RemovalPort` whose removal cannot even be waited on: [`Detached::wait`]'s other failure,
/// where `wait_with_output` itself fails and the context names the removal over whatever the
/// OS said. The child may still be running. Worded as the adapter words it, because what
/// reaches the prompt line is the whole chain or none of it.
pub struct Unwaited;

impl RemovalPort for Unwaited {
    fn start(
        &self,
        _repo_root: &str,
        _checkout_path: &str,
        label: &str,
        _panes_closed: usize,
        _delete_branch: bool,
    ) -> Result<Box<dyn RunningRemoval>> {
        Ok(Box::new(Unreaped(label.to_string())))
    }
}

struct Unreaped(String);

impl RunningRemoval for Unreaped {
    fn wait(self: Box<Self>) -> Result<RemovalOutcome> {
        Err(anyhow!("No child processes (os error 10)")
            .context(format!("waiting for the removal of {}", self.0)))
    }
}

/// A `RemovalPort` whose first removal ends without a readable word and whose every one after
/// reports that it went: one frame with both shapes of report in it.
pub struct LostFirst {
    asked: AtomicBool,
}

impl Default for LostFirst {
    fn default() -> Self {
        Self {
            asked: AtomicBool::new(false),
        }
    }
}

impl RemovalPort for LostFirst {
    fn start(
        &self,
        repo_root: &str,
        checkout_path: &str,
        label: &str,
        panes_closed: usize,
        delete_branch: bool,
    ) -> Result<Box<dyn RunningRemoval>> {
        match self.asked.swap(true, Ordering::SeqCst) {
            false => Lost.start(repo_root, checkout_path, label, panes_closed, delete_branch),
            true => Reports(RemovalOutcome::Removed).start(
                repo_root,
                checkout_path,
                label,
                panes_closed,
                delete_branch,
            ),
        }
    }
}

/// A `RemovalPort` that will not start anything. The branch it exercises is the worst one
/// in [`Removals::remove`]: every pane is already closed by the time it is reached.
pub struct Refuses;

impl RemovalPort for Refuses {
    fn start(
        &self,
        _repo_root: &str,
        _checkout_path: &str,
        _label: &str,
        _panes_closed: usize,
        _delete_branch: bool,
    ) -> Result<Box<dyn RunningRemoval>> {
        Err(anyhow!("could not spawn: no such file or directory"))
    }
}

/// What a fake in this layer answers, and what it therefore refuses.
///
/// Every method here refuses, so an `impl FakeGit for …` block *is* the list of what that
/// fake's port is asked for: a method missing from it is a call that fails the test rather
/// than one that hands back a value the test then goes on asserting about. That is the same
/// rule [`Recorder`] is written to, moved out of each fake's own `impl` block and into one
/// place — a port that grows a method grows it here and in [`fake_git`], and no fake has to
/// be visited to go on refusing it.
pub trait FakeGit {
    fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
        refused("GitPort::identify")
    }
    fn github_slug(&self, _repo_root: &str) -> Result<Option<crate::port::Slug>> {
        refused("GitPort::github_slug")
    }
    fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
        refused("GitPort::local_refs")
    }
    fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
        refused("GitPort::remote_heads")
    }
    fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
        refused("GitPort::fetch_branch")
    }
    fn fetch_all(&self, _repo_root: &str) -> Result<()> {
        refused("GitPort::fetch_all")
    }
    fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
        refused("GitPort::remove_worktree")
    }
    fn delete_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
        refused("GitPort::delete_branch")
    }
    fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
        refused("GitPort::is_dirty")
    }
    fn head_ref(&self, _repo_root: &str) -> Result<String> {
        refused("GitPort::head_ref")
    }
}

/// The `gh` half of the same arrangement — see [`FakeGit`].
pub trait FakeGh {
    fn pull_requests(&self, _slug: &crate::port::Slug) -> Vec<crate::port::PullRequest> {
        refused("GhPort::pull_requests")
    }
    fn settled_pull_requests(
        &self,
        _slug: &crate::port::Slug,
    ) -> std::result::Result<crate::port::SettledPullRequests, String> {
        refused("GhPort::settled_pull_requests")
    }
}

/// The herdr half of the same arrangement — see [`FakeGit`].
pub trait FakeHerdr {
    fn snapshot(&self) -> Result<crate::port::Snapshot> {
        refused("HerdrPort::snapshot")
    }
    fn worktree_list(&self, _cwd: &str) -> Result<crate::port::WorktreeList> {
        refused("HerdrPort::worktree_list")
    }
    fn worktree_create(&self, _req: &WorktreeCreate) -> Result<WorktreeOpened> {
        refused("HerdrPort::worktree_create")
    }
    fn worktree_open(&self, _req: &WorktreeOpen) -> Result<WorktreeOpened> {
        refused("HerdrPort::worktree_open")
    }
    fn pane_focus(&self, _pane_id: &str) -> Result<()> {
        refused("HerdrPort::pane_focus")
    }
    fn pane_split(&self, _req: &PaneSplit) -> Result<Pane> {
        refused("HerdrPort::pane_split")
    }
    fn pane_close(&self, _pane_id: &str) -> Result<()> {
        refused("HerdrPort::pane_close")
    }
    fn pane_move(&self, _pane_id: &str, _dest: &PaneDestination, _focus: bool) -> Result<()> {
        refused("HerdrPort::pane_move")
    }
    fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
        refused("HerdrPort::workspace_focus")
    }
    fn tab_focus(&self, _tab_id: &str) -> Result<()> {
        refused("HerdrPort::tab_focus")
    }
    fn plugin_pane_open(&self, _req: &PluginPaneOpen) -> Result<Option<OpenRefusal>> {
        refused("HerdrPort::plugin_pane_open")
    }
    fn notify(&self, _notification: &Notification) -> Result<()> {
        refused("HerdrPort::notify")
    }
}

/// What every unanswered method does. `#[track_caller]` so the panic names the call the
/// test made rather than this line.
#[track_caller]
fn refused(method: &str) -> ! {
    unreachable!("{method} is not one of the answers this fake gives")
}

/// Make what `$fake` answers a `GitPort`.
macro_rules! fake_git {
    ($fake:ty) => {
        impl $crate::port::GitPort for $fake {
            fn identify(&self, cwd: &str) -> ::anyhow::Result<Option<$crate::port::RepoIdentity>> {
                $crate::app::fakes::FakeGit::identify(self, cwd)
            }
            fn github_slug(&self, repo_root: &str) -> ::anyhow::Result<Option<$crate::port::Slug>> {
                $crate::app::fakes::FakeGit::github_slug(self, repo_root)
            }
            fn local_refs(&self, repo_root: &str) -> ::anyhow::Result<$crate::port::RefWalk> {
                $crate::app::fakes::FakeGit::local_refs(self, repo_root)
            }
            fn remote_heads(&self, repo_root: &str) -> ::anyhow::Result<Vec<String>> {
                $crate::app::fakes::FakeGit::remote_heads(self, repo_root)
            }
            fn fetch_branch(&self, repo_root: &str, branch: &str) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeGit::fetch_branch(self, repo_root, branch)
            }
            fn fetch_all(&self, repo_root: &str) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeGit::fetch_all(self, repo_root)
            }
            fn remove_worktree(
                &self,
                repo_root: &str,
                checkout_path: &str,
            ) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeGit::remove_worktree(self, repo_root, checkout_path)
            }
            fn delete_branch(&self, repo_root: &str, branch: &str) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeGit::delete_branch(self, repo_root, branch)
            }
            fn is_dirty(&self, checkout_path: &str) -> ::anyhow::Result<bool> {
                $crate::app::fakes::FakeGit::is_dirty(self, checkout_path)
            }
            fn head_ref(&self, repo_root: &str) -> ::anyhow::Result<String> {
                $crate::app::fakes::FakeGit::head_ref(self, repo_root)
            }
        }
    };
}

/// Make what `$fake` answers a `GhPort` — see [`fake_git`].
macro_rules! fake_gh {
    ($fake:ty) => {
        impl $crate::port::GhPort for $fake {
            fn pull_requests(&self, slug: &$crate::port::Slug) -> Vec<$crate::port::PullRequest> {
                $crate::app::fakes::FakeGh::pull_requests(self, slug)
            }
            fn settled_pull_requests(
                &self,
                slug: &$crate::port::Slug,
            ) -> ::std::result::Result<$crate::port::SettledPullRequests, String> {
                $crate::app::fakes::FakeGh::settled_pull_requests(self, slug)
            }
        }
    };
}

/// Make what `$fake` answers a `HerdrPort` — see [`fake_git`].
macro_rules! fake_herdr {
    ($fake:ty) => {
        impl $crate::port::HerdrPort for $fake {
            fn snapshot(&self) -> ::anyhow::Result<$crate::port::Snapshot> {
                $crate::app::fakes::FakeHerdr::snapshot(self)
            }
            fn worktree_list(&self, cwd: &str) -> ::anyhow::Result<$crate::port::WorktreeList> {
                $crate::app::fakes::FakeHerdr::worktree_list(self, cwd)
            }
            fn worktree_create(
                &self,
                req: &$crate::port::WorktreeCreate,
            ) -> ::anyhow::Result<$crate::port::WorktreeOpened> {
                $crate::app::fakes::FakeHerdr::worktree_create(self, req)
            }
            fn worktree_open(
                &self,
                req: &$crate::port::WorktreeOpen,
            ) -> ::anyhow::Result<$crate::port::WorktreeOpened> {
                $crate::app::fakes::FakeHerdr::worktree_open(self, req)
            }
            fn pane_focus(&self, pane_id: &str) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeHerdr::pane_focus(self, pane_id)
            }
            fn pane_split(
                &self,
                req: &$crate::port::PaneSplit,
            ) -> ::anyhow::Result<$crate::port::Pane> {
                $crate::app::fakes::FakeHerdr::pane_split(self, req)
            }
            fn pane_close(&self, pane_id: &str) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeHerdr::pane_close(self, pane_id)
            }
            fn pane_move(
                &self,
                pane_id: &str,
                dest: &$crate::port::PaneDestination,
                focus: bool,
            ) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeHerdr::pane_move(self, pane_id, dest, focus)
            }
            fn workspace_focus(&self, workspace_id: &str) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeHerdr::workspace_focus(self, workspace_id)
            }
            fn tab_focus(&self, tab_id: &str) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeHerdr::tab_focus(self, tab_id)
            }
            fn plugin_pane_open(
                &self,
                req: &$crate::port::PluginPaneOpen,
            ) -> ::anyhow::Result<Option<$crate::port::OpenRefusal>> {
                $crate::app::fakes::FakeHerdr::plugin_pane_open(self, req)
            }
            fn notify(&self, notification: &$crate::port::Notification) -> ::anyhow::Result<()> {
                $crate::app::fakes::FakeHerdr::notify(self, notification)
            }
        }
    };
}

pub(crate) use {fake_gh, fake_git, fake_herdr};
