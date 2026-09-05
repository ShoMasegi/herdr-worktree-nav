//! What the tests in this layer drive their ports with, and wait on.
//!
//! Here rather than in one module's `mod tests` because the ordering rule these exist to
//! pin — panes close, then the removal starts — spans two ports. Both write into one
//! `Recorder` log, so a test reads a single interleaving rather than two sequences it has
//! to merge by eye. That is also why both ports live in this file: `record` is private, so
//! the grammar of that log is owned here — a line can only be put into it by one of the two
//! ports below, saying one of the two things they say.

use std::sync::Mutex;

use anyhow::{anyhow, Result};

use crate::port::{
    HerdrPort, Notification, OpenRefusal, Pane, PaneDestination, PaneSplit, PluginPaneOpen,
    RemovalOutcome, RemovalPort, RunningRemoval, Snapshot, WorktreeCreate, WorktreeList,
    WorktreeOpen, WorktreeOpened,
};

/// Spin until `ready`, or fail the test.
///
/// The work these tests drive runs on threads of its own, so there is nothing to join on and
/// nothing to block for. Two seconds is far longer than any of them needs and short enough
/// to notice; `what` is what makes the difference between "this hung" and "this machine is
/// loaded" readable when it does fire. Here rather than in each module so that the budget is
/// one number — every caller in this layer goes through it, including the ones that are
/// waiting for a condition to stop holding.
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
/// needs one of them adds a fake of its own — filling one in here would quietly weaken
/// every test already using it, and none of them would fail to say so.
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

    /// A herdr that will not describe itself. The only thing asked of this fake besides
    /// `pane_close`, and it refuses on purpose: what a caller does when the panes have
    /// closed and the list cannot be read again is a decision worth pinning, and it is
    /// unreachable from a test if this panics instead of answering.
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
    ) -> Result<Box<dyn RunningRemoval>> {
        self.0
            .record(format!("start {checkout_path} after {panes_closed}"));
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
    ) -> Result<Box<dyn RunningRemoval>> {
        Err(anyhow!("could not spawn: no such file or directory"))
    }
}
