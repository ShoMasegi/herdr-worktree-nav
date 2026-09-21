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
    HerdrPort, Notification, OpenRefusal, Pane, PaneDestination, PaneSplit, PluginPaneOpen,
    RemovalOutcome, RemovalPort, RunningRemoval, Snapshot, WorktreeCreate, WorktreeList,
    WorktreeOpen, WorktreeOpened,
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
/// half of that arm needs a herdr that answers — `app::panes`'s `Closing`.
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

impl HerdrPort for Recorder {
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
    fn worktree_list(&self, _cwd: &str) -> Result<WorktreeList> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
    fn worktree_create(&self, _req: &WorktreeCreate) -> Result<WorktreeOpened> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
    fn worktree_open(&self, _req: &WorktreeOpen) -> Result<WorktreeOpened> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
    fn pane_focus(&self, _pane_id: &str) -> Result<()> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
    fn pane_split(&self, _req: &PaneSplit) -> Result<Pane> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
    fn pane_move(&self, _pane: &str, _dest: &PaneDestination, _focus: bool) -> Result<()> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
    fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
    fn tab_focus(&self, _tab_id: &str) -> Result<()> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
    fn plugin_pane_open(&self, _req: &PluginPaneOpen) -> Result<Option<OpenRefusal>> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
    fn notify(&self, _notification: &Notification) -> Result<()> {
        unreachable!("only pane_close is asked of Recorder's HerdrPort")
    }
}

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

/// A `RemovalPort` whose every removal ends without a word this side can read: the last arm
/// of `adapter::detached::Detached::wait`, where nothing a report could be parsed out of came
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

/// A `RemovalPort` whose removal cannot even be waited on: `Detached::wait`'s other failure,
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
/// in `Removals::remove`: every pane is already closed by the time it is reached.
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
