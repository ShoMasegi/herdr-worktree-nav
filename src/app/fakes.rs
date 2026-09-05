//! Ports that record what they were asked to do, for the tests in this layer.
//!
//! Shared because more than one module needs the same one: the ordering rule these exist to
//! pin — panes close, then the removal starts — now spans two of them.

use std::sync::Mutex;

use anyhow::{anyhow, Result};

use crate::port::{
    HerdrPort, Notification, OpenRefusal, Pane, PaneDestination, PaneSplit, PluginPaneOpen,
    Snapshot, WorktreeCreate, WorktreeList, WorktreeOpen, WorktreeOpened,
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
/// pane. Everything it is not asked in these tests is `unreachable!()`, so a call that was
/// not meant to happen fails loudly rather than quietly returning something plausible.
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

    /// Add to the same log the pane closes go into, so a test can assert one interleaving
    /// rather than two independent sequences.
    pub fn record(&self, what: String) {
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
    fn pane_split(&self, _req: &PaneSplit) -> Result<Pane> {
        unreachable!()
    }
    fn pane_move(&self, _pane: &str, _dest: &PaneDestination, _focus: bool) -> Result<()> {
        unreachable!()
    }
    fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
        unreachable!()
    }
    fn tab_focus(&self, _tab_id: &str) -> Result<()> {
        unreachable!()
    }
    fn plugin_pane_open(&self, _req: &PluginPaneOpen) -> Result<Option<OpenRefusal>> {
        unreachable!()
    }
    fn notify(&self, _notification: &Notification) -> Result<()> {
        unreachable!()
    }
}
