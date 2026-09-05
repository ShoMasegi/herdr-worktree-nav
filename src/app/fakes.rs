//! What the tests in this layer drive their ports with, and wait on.
//!
//! Here rather than in one module's `mod tests` because the ordering rule these exist to
//! pin — panes close, then the removal starts — spans two ports. Both write into one
//! `Recorder` log, so a test reads a single interleaving rather than two sequences it has
//! to merge by eye. That is also why the whole vocabulary of that log lives in this file:
//! `record` is private, so no fake elsewhere can put a line into it that reads like one of
//! these.

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
/// nothing to block for. The budget is a thousand times what any of them has been measured to
/// need, and it is here rather than in each module so that raising it is one edit.
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

    fn snapshot(&self) -> Result<Snapshot> {
        unreachable!("only pane_close is asked of this port")
    }
    fn worktree_list(&self, _cwd: &str) -> Result<WorktreeList> {
        unreachable!("only pane_close is asked of this port")
    }
    fn worktree_create(&self, _req: &WorktreeCreate) -> Result<WorktreeOpened> {
        unreachable!("only pane_close is asked of this port")
    }
    fn worktree_open(&self, _req: &WorktreeOpen) -> Result<WorktreeOpened> {
        unreachable!("only pane_close is asked of this port")
    }
    fn pane_focus(&self, _pane_id: &str) -> Result<()> {
        unreachable!("only pane_close is asked of this port")
    }
    fn pane_split(&self, _req: &PaneSplit) -> Result<Pane> {
        unreachable!("only pane_close is asked of this port")
    }
    fn pane_move(&self, _pane: &str, _dest: &PaneDestination, _focus: bool) -> Result<()> {
        unreachable!("only pane_close is asked of this port")
    }
    fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
        unreachable!("only pane_close is asked of this port")
    }
    fn tab_focus(&self, _tab_id: &str) -> Result<()> {
        unreachable!("only pane_close is asked of this port")
    }
    fn plugin_pane_open(&self, _req: &PluginPaneOpen) -> Result<Option<OpenRefusal>> {
        unreachable!("only pane_close is asked of this port")
    }
    fn notify(&self, _notification: &Notification) -> Result<()> {
        unreachable!("only pane_close is asked of this port")
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
