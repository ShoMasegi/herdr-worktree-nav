//! Which checkouts are holding uncommitted work.
//!
//! The one answer in the panes view that cannot ride on a call already being made: git has
//! to walk a working tree to know it, once per checkout. So it is asked behind the first
//! frame and each row is filled in as its answer lands — the shape
//! `docs/adr/0007-stay-up-while-working.md` established for the remote listing, and for the
//! same reason: a picker that waits for git is a picker that looks broken.
//!
//! A checkout that has not answered yet is drawn with no marker rather than with a guess.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use crate::domain::model::Tree;
use crate::port::GitPort;

/// Enough to hide the latency without filling a laptop with git processes. The same cap the
/// working-directory resolution uses, for the same reason.
const MAX_IN_FLIGHT: usize = 8;

/// What the picker knows about uncommitted work, and what it is still waiting to hear.
///
/// Owned by the view switch, so an answer is asked for once and then kept for as long as the
/// picker is up — `Tab` away and back is a frame, not another walk of every working tree.
pub struct Dirty {
    git: Arc<dyn GitPort>,
    sender: Sender<(String, bool)>,
    receiver: Receiver<(String, bool)>,
    /// Every checkout that has been asked about, answered or not, so none is asked twice.
    asked: HashSet<String>,
    /// Waiting for a slot.
    queued: VecDeque<String>,
    in_flight: usize,
    /// The ones git said are dirty. Clean and not-yet-answered are the same thing here,
    /// because they draw the same.
    dirty: Vec<String>,
}

impl Dirty {
    pub fn new(git: Arc<dyn GitPort>) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            git,
            sender,
            receiver,
            asked: HashSet::new(),
            queued: VecDeque::new(),
            in_flight: 0,
            dirty: Vec::new(),
        }
    }

    /// Ask about every checkout in the tree that has not been asked about yet.
    pub fn ask(&mut self, tree: &Tree) {
        for repo in &tree.repos {
            for worktree in &repo.worktrees {
                if self.asked.insert(worktree.checkout_path.clone()) {
                    self.queued.push_back(worktree.checkout_path.clone());
                }
            }
        }
        self.pump();
    }

    /// Forget everything and ask again. What `r` means: the answers are a snapshot of
    /// working trees the user has been editing since.
    pub fn forget(&mut self) {
        self.asked.clear();
        self.queued.clear();
        self.dirty.clear();
    }

    /// Take in whatever has arrived. `true` when the marked set changed and the rows need
    /// rebuilding.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok((checkout_path, dirty)) = self.receiver.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            // An answer for a checkout that has since been forgotten is dropped: `forget`
            // cleared `asked`, so it will be asked again and answered again.
            if dirty && self.asked.contains(&checkout_path) && !self.dirty.contains(&checkout_path)
            {
                self.dirty.push(checkout_path);
                changed = true;
            }
        }
        self.pump();
        changed
    }

    /// The checkouts known to be holding uncommitted work.
    pub fn paths(&self) -> Vec<String> {
        self.dirty.clone()
    }

    /// Whether any answer is still coming. The loop turns a spinner while this is true.
    pub fn is_waiting(&self) -> bool {
        self.in_flight > 0 || !self.queued.is_empty()
    }

    fn pump(&mut self) {
        while self.in_flight < MAX_IN_FLIGHT {
            let Some(checkout_path) = self.queued.pop_front() else {
                return;
            };
            self.in_flight += 1;
            let git = Arc::clone(&self.git);
            let sender = self.sender.clone();
            // Not joined anywhere. These outlive the view that asked — the answers are
            // wanted on both sides of a `Tab` — and leaving the picker ends them with the
            // process, which costs nothing: `git status` writes nothing.
            std::thread::spawn(move || {
                // A checkout git could not answer for is not a checkout with something in
                // it. No marker beats the wrong marker.
                let dirty = git.is_dirty(&checkout_path).unwrap_or(false);
                let _ = sender.send((checkout_path, dirty));
            });
        }
    }
}
