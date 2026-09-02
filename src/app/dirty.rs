//! Which checkouts are holding uncommitted work.
//!
//! The one answer in the panes view that cannot ride on a call already being made: git has
//! to walk a working tree to know it, once per checkout. So it is asked behind the first
//! frame and each row is filled in as its answer lands — the shape
//! `docs/adr/0009-the-picker-owns-the-terminal.md` established for the remote listing, and
//! for the same two reasons: a picker that waits for git is a picker that looks broken, and
//! an answer that cost a round of processes should survive `Tab` rather than be asked again.
//!
//! A checkout that has not answered yet is drawn with no marker rather than with a guess.

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use crate::domain::model::Tree;
use crate::port::GitPort;

/// Enough to hide the latency without filling a laptop with git processes. The same cap the
/// working-directory resolution uses, for the same reason.
const MAX_IN_FLIGHT: usize = 8;

/// One answer, tagged with the round of asking it belongs to.
type Answer = (u64, String, bool);

/// What the picker knows about uncommitted work, and what it is still waiting to hear.
///
/// Owned by the view switch, so an answer is asked for once and then kept for as long as the
/// picker is up — `Tab` away and back is a frame, not another walk of every working tree.
pub struct Dirty {
    git: Arc<dyn GitPort>,
    sender: Sender<Answer>,
    receiver: Receiver<Answer>,
    /// Which round of asking is current. Bumped by [`forget`](Self::forget), because a
    /// `git status` started before it was called is answering about a working tree the user
    /// has since changed — that is the whole reason they pressed the key.
    generation: u64,
    /// Every checkout asked about in the current round: `None` until git answers, then
    /// whether it is holding anything. Keeping the clean answers as well as the dirty ones
    /// is what lets a second answer correct a first.
    answers: BTreeMap<String, Option<bool>>,
    /// Asked for, waiting on a slot.
    queued: VecDeque<String>,
    in_flight: usize,
}

impl Dirty {
    pub fn new(git: Arc<dyn GitPort>) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            git,
            sender,
            receiver,
            generation: 0,
            answers: BTreeMap::new(),
            queued: VecDeque::new(),
            in_flight: 0,
        }
    }

    /// Ask about every checkout in the tree that has not been asked about yet.
    pub fn ask(&mut self, tree: &Tree) {
        for repo in &tree.repos {
            for worktree in &repo.worktrees {
                if !self.answers.contains_key(&worktree.checkout_path) {
                    self.answers.insert(worktree.checkout_path.clone(), None);
                    self.queued.push_back(worktree.checkout_path.clone());
                }
            }
        }
        self.pump();
    }

    /// Throw away every answer and every question. The caller asks again; what it gets back
    /// is about the working trees as they are now.
    ///
    /// Threads already running are left alone — there is no way to call one back — but their
    /// answers belong to the round this ends, and [`drain`](Self::drain) drops them on that
    /// basis rather than on whether the checkout is still listed.
    pub fn forget(&mut self) {
        self.generation += 1;
        self.answers.clear();
        self.queued.clear();
    }

    /// Take in whatever has arrived. `true` when the marked set changed and the rows need
    /// rebuilding.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok((generation, checkout_path, dirty)) = self.receiver.try_recv() {
            // Counted whatever round it came from: it is a thread that has finished, and
            // the cap and the spinner are both about how many are still running.
            self.in_flight = self.in_flight.saturating_sub(1);
            if generation != self.generation {
                continue;
            }
            if let Some(answer) = self.answers.get_mut(&checkout_path) {
                changed |= *answer != Some(dirty);
                *answer = Some(dirty);
            }
        }
        self.pump();
        changed
    }

    /// The checkouts known to be holding uncommitted work. In path order, so that a caller
    /// comparing this against what it drew last does not see the order threads happened to
    /// finish in as a change.
    pub fn paths(&self) -> Vec<String> {
        self.answers
            .iter()
            .filter(|(_, answer)| **answer == Some(true))
            .map(|(checkout_path, _)| checkout_path.clone())
            .collect()
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
            let generation = self.generation;
            // Not joined anywhere. These outlive the view that asked — the answers are
            // wanted on both sides of a `Tab` — and leaving the picker ends the threads with
            // the process. The `git status` each one is waiting on is a child process that
            // carries on to its own end; it is read-only about anything the user would
            // notice, and `--no-optional-locks` keeps it from touching the index.
            std::thread::spawn(move || {
                // A checkout git could not answer for is not a checkout with something in
                // it. No marker beats the wrong marker.
                let dirty = git.is_dirty(&checkout_path).unwrap_or(false);
                let _ = sender.send((generation, checkout_path, dirty));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{RepoNode, WorktreeNode};
    use crate::port::{GitRef, RepoIdentity};
    use anyhow::Result;
    use std::sync::Mutex;

    /// A `GitPort` whose `is_dirty` blocks until the test releases it, so the window this
    /// module exists to manage — an answer in flight while the user does something else —
    /// can be opened and closed deliberately.
    struct FakeGit {
        answered: Mutex<Vec<(String, std::sync::mpsc::Sender<bool>)>>,
    }

    impl FakeGit {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                answered: Mutex::new(Vec::new()),
            })
        }

        /// Answer the oldest outstanding call for this checkout.
        fn answer(&self, checkout_path: &str, dirty: bool) {
            let mut waiting = self.answered.lock().unwrap();
            let index = waiting
                .iter()
                .position(|(path, _)| path == checkout_path)
                .unwrap_or_else(|| panic!("nothing asked about {checkout_path}"));
            let (_, reply) = waiting.remove(index);
            reply.send(dirty).expect("the worker should still be there");
        }

        fn outstanding(&self) -> usize {
            self.answered.lock().unwrap().len()
        }
    }

    impl GitPort for FakeGit {
        fn is_dirty(&self, checkout_path: &str) -> Result<bool> {
            let (reply, wait) = mpsc::channel();
            self.answered
                .lock()
                .unwrap()
                .push((checkout_path.to_string(), reply));
            Ok(wait.recv().unwrap_or(false))
        }

        fn identify(&self, _cwd: &str) -> Result<Option<RepoIdentity>> {
            unreachable!("only is_dirty is asked of this port")
        }
        fn github_slug(&self, _repo_root: &str) -> Result<Option<String>> {
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
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!()
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!()
        }
    }

    fn tree(checkouts: &[&str]) -> Tree {
        Tree {
            repos: vec![RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                worktrees: checkouts
                    .iter()
                    .map(|path| WorktreeNode {
                        branch: Some("b".into()),
                        checkout_path: (*path).to_string(),
                        is_primary: false,
                        open_workspace_id: None,
                        track: None,
                        panes: Vec::new(),
                    })
                    .collect(),
            }],
            ungrouped: Vec::new(),
        }
    }

    /// Wait for the worker threads to have asked, since they run on their own clock.
    fn until_asked(git: &FakeGit, count: usize) {
        for _ in 0..2000 {
            if git.outstanding() >= count {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("expected {count} checkouts to be asked about");
    }

    #[test]
    fn an_answer_from_before_a_reload_is_not_taken_for_a_fresh_one() {
        // The case the manual checklist asks a tester to confirm: leave uncommitted work,
        // let the walk start, commit it, press `r`. The walk that is still running knows
        // only about the working tree as it was, and its answer must not come back as the
        // answer to the question the user just asked.
        let git = FakeGit::new();
        let mut dirty = Dirty::new(git.clone());
        let tree = tree(&["/wt/a"]);

        dirty.ask(&tree);
        until_asked(&git, 1);

        dirty.forget();
        dirty.ask(&tree);
        until_asked(&git, 2);

        // The stale answer lands first, and says what was true before the reload.
        git.answer("/wt/a", true);
        for _ in 0..200 {
            dirty.drain();
            if !dirty.is_waiting() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            dirty.paths().is_empty(),
            "the answer belonged to a question that was withdrawn"
        );

        git.answer("/wt/a", false);
        until_answered(&mut dirty);
        assert!(dirty.paths().is_empty(), "and the fresh answer is clean");
    }

    /// Drain until nothing is outstanding.
    fn until_answered(dirty: &mut Dirty) {
        for _ in 0..2000 {
            dirty.drain();
            if !dirty.is_waiting() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("answers never arrived");
    }

    #[test]
    fn a_checkout_that_has_been_cleaned_stops_being_marked() {
        // Within one round an answer is final, but across a reload the second answer has to
        // be able to undo the first — otherwise a marker can only ever be added.
        let git = FakeGit::new();
        let mut dirty = Dirty::new(git.clone());
        let tree = tree(&["/wt/a"]);

        dirty.ask(&tree);
        until_asked(&git, 1);
        git.answer("/wt/a", true);
        until_answered(&mut dirty);
        assert_eq!(dirty.paths(), vec!["/wt/a".to_string()]);

        dirty.forget();
        dirty.ask(&tree);
        until_asked(&git, 1);
        git.answer("/wt/a", false);
        until_answered(&mut dirty);
        assert!(dirty.paths().is_empty());
    }

    #[test]
    fn a_checkout_is_asked_about_once_however_often_the_view_comes_back() {
        // Which is the whole reason this is owned by the view switch rather than by the
        // panes view: `Tab` away and back must not walk every working tree again.
        let git = FakeGit::new();
        let mut dirty = Dirty::new(git.clone());
        let tree = tree(&["/wt/a", "/wt/b"]);

        dirty.ask(&tree);
        until_asked(&git, 2);
        dirty.ask(&tree);
        dirty.ask(&tree);
        assert_eq!(git.outstanding(), 2, "asked once each, not once per visit");

        git.answer("/wt/a", true);
        git.answer("/wt/b", false);
        until_answered(&mut dirty);
        dirty.ask(&tree);
        assert_eq!(
            git.outstanding(),
            0,
            "and not again once they have answered"
        );
        assert_eq!(dirty.paths(), vec!["/wt/a".to_string()]);
    }

    #[test]
    fn no_more_than_eight_working_trees_are_walked_at_once() {
        // A user with forty worktrees is the reason: forty `git status` processes at once
        // is a laptop that stops for a moment.
        let git = FakeGit::new();
        let mut dirty = Dirty::new(git.clone());
        let paths: Vec<String> = (0..20).map(|n| format!("/wt/{n}")).collect();
        let borrowed: Vec<&str> = paths.iter().map(String::as_str).collect();

        dirty.ask(&tree(&borrowed));
        until_asked(&git, MAX_IN_FLIGHT);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(git.outstanding(), MAX_IN_FLIGHT);

        for path in paths.iter().take(MAX_IN_FLIGHT) {
            git.answer(path, false);
        }
        // The next slots open on the drain, which is where the loop takes answers in.
        for _ in 0..2000 {
            dirty.drain();
            if git.outstanding() >= MAX_IN_FLIGHT {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            git.outstanding(),
            MAX_IN_FLIGHT,
            "the rest follow as slots come free, and no faster"
        );
        assert!(dirty.is_waiting());
    }
}
