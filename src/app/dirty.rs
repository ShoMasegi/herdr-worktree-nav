//! Which checkouts are holding uncommitted work.
//!
//! The one answer in the panes view that cannot ride on a call already being made: git has
//! to walk a working tree to know it, once per checkout. So it is asked behind the first
//! frame and each row is filled in as its answer lands, for the reason
//! `docs/adr/0007-stay-up-while-working.md` gives — a picker that waits for git is a picker
//! that looks broken — and it is owned by the view switch rather than by the view, for the
//! reason `docs/adr/0009-the-picker-owns-the-terminal.md` gives about the remote listing: an
//! answer that cost a round of processes should survive `Tab` rather than be asked again.
//!
//! A checkout that has not answered yet is drawn with no marker rather than with a guess.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use crate::domain::model::{Tree, WorkingTree};
use crate::port::GitPort;

/// Enough to hide the latency without filling a laptop with git processes. The same cap the
/// working-directory resolution uses, for the same reason.
const MAX_IN_FLIGHT: usize = 8;

/// One answer, tagged with the round of asking it belongs to. `None` is git declining to
/// answer at all.
type Reply = (u64, String, Option<bool>);

/// What the picker knows about uncommitted work, and what it is still waiting to hear.
///
/// Owned by the view switch, so an answer is asked for once and then kept for as long as the
/// picker is up — `Tab` away and back is a frame, not another walk of every working tree.
pub struct Dirty {
    git: Arc<dyn GitPort>,
    sender: Sender<Reply>,
    receiver: Receiver<Reply>,
    /// Which round of asking is current. Bumped by [`forget`](Self::forget), because a
    /// `git status` started before it was called is answering about a working tree the user
    /// has since changed — that is the whole reason they pressed the key.
    generation: u64,
    /// Every checkout asked about in the current round. Keeping the clean answers as well
    /// as the dirty ones is what lets a second answer correct a first.
    /// `None` for a checkout that has been asked and has not answered.
    answers: BTreeMap<String, Option<WorkingTree>>,
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

    /// Ask about every checkout in the tree that has not been asked about yet, and forget
    /// the ones that have left it.
    ///
    /// Forgetting matters because this is kept for the life of the picker: without it a
    /// session's worth of deleted checkouts accumulates, and every one of them is an answer
    /// about a working tree that is no longer there.
    pub fn ask(&mut self, tree: &Tree) {
        let listed: BTreeSet<&str> = tree
            .repos
            .iter()
            .flat_map(|repo| &repo.worktrees)
            .map(|worktree| worktree.checkout_path.as_str())
            .collect();
        self.answers
            .retain(|path, _| listed.contains(path.as_str()));
        self.queued.retain(|path| listed.contains(path.as_str()));

        // In tree order rather than in the set's, so the walk fills the list in from the
        // top — which is where the reader is.
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

    /// Throw every answer away and ask again. What comes back is about the working trees as
    /// they are now, which is what `r` means about a tree the user has been editing since.
    ///
    /// Threads already running are left alone — there is no way to call one back — but their
    /// answers belong to the round this ends, and [`drain`](Self::drain) drops them on that
    /// basis rather than on whether the checkout is still listed. Asking is not separable
    /// from forgetting: a `Dirty` that had forgotten and not yet asked would sit with its
    /// spinner turning over a list it will never say anything about.
    pub fn reask(&mut self, tree: &Tree) {
        self.generation += 1;
        self.answers.clear();
        self.queued.clear();
        self.ask(tree);
    }

    /// Take in whatever has arrived, and start whatever the freed slots allow.
    ///
    /// Says nothing about whether anything needs redrawing. Which answers are worth a rebuild
    /// is a question about rows, and it is asked where the rows are —
    /// `ui::state::PanesState::set_working_trees`. Here every answer is equal.
    ///
    /// This is also the pump, so a view that stops draining stops the walk: with more
    /// checkouts than `MAX_IN_FLIGHT`, the remainder waits for the panes view to come back.
    pub fn drain(&mut self) {
        while let Ok((generation, checkout_path, dirty)) = self.receiver.try_recv() {
            // Counted whatever round it came from: it is a thread that has finished, and
            // the cap and the spinner are both about how many are still running.
            self.in_flight = self.in_flight.saturating_sub(1);
            if generation != self.generation {
                continue;
            }
            let answered = Some(match dirty {
                Some(true) => WorkingTree::Dirty,
                Some(false) => WorkingTree::Clean,
                None => WorkingTree::Unreadable,
            });
            if let Some(answer) = self.answers.get_mut(&checkout_path) {
                *answer = answered;
            }
        }
        self.pump();
    }

    /// What git has said so far, by checkout. A checkout that has been asked and not yet
    /// answered is absent rather than present with a guess, so "nobody knows" and "clean"
    /// stay different facts all the way to the caller — which is what
    /// `ui::state::PanesState::ask_to_remove` refuses on, and what
    /// `docs/adr/0011-what-may-be-swept.md` decides on.
    ///
    /// In path order, so a caller comparing this against what it drew last does not see the
    /// order threads happened to finish in as a change.
    pub fn answers(&self) -> BTreeMap<String, WorkingTree> {
        self.answers
            .iter()
            .filter_map(|(path, answer)| Some((path.clone(), (*answer)?)))
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
                // A checkout git could not answer for gets no marker — no marker beats the
                // wrong marker — but it is not recorded as clean, because that is a claim
                // and this is the absence of one.
                let dirty = git.is_dirty(&checkout_path).ok();
                let _ = sender.send((generation, checkout_path, dirty));
            });
        }
    }
}

#[cfg(test)]
mod tests {

    /// The three questions these tests ask of the walk, projected out of the one map it now
    /// hands back. Kept here rather than on `Dirty` because nothing in the picker wants them
    /// separately any more — telling them apart at the call site is what this replaced.
    fn dirty_paths(walk: &Dirty) -> Vec<String> {
        picked(walk, |answer| answer == WorkingTree::Dirty)
    }

    fn unreadable(walk: &Dirty) -> Vec<String> {
        picked(walk, |answer| answer == WorkingTree::Unreadable)
    }

    fn answered(walk: &Dirty) -> Vec<String> {
        picked(walk, |_| true)
    }

    fn picked(walk: &Dirty, keep: impl Fn(WorkingTree) -> bool) -> Vec<String> {
        walk.answers()
            .into_iter()
            .filter(|(_, answer)| keep(*answer))
            .map(|(path, _)| path)
            .collect()
    }
    use super::*;
    use crate::app::fakes::until;
    use crate::domain::model::{RepoNode, WorktreeNode};
    use crate::port::{GitRef, RepoIdentity};
    use anyhow::Result;
    use std::sync::Mutex;

    /// A `GitPort` whose `is_dirty` blocks until the test releases it, so the window this
    /// module exists to manage — an answer in flight while the user does something else —
    /// can be opened and closed deliberately.
    struct FakeGit {
        answered: Mutex<Vec<(String, std::sync::mpsc::Sender<Option<bool>>)>>,
    }

    impl FakeGit {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                answered: Mutex::new(Vec::new()),
            })
        }

        /// Answer the oldest outstanding call for this checkout.
        fn answer(&self, checkout_path: &str, dirty: bool) {
            self.reply(checkout_path, Some(dirty));
        }

        /// Refuse it, the way git refuses a repository it will not touch.
        fn refuse(&self, checkout_path: &str) {
            self.reply(checkout_path, None);
        }

        fn reply(&self, checkout_path: &str, dirty: Option<bool>) {
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
            match wait.recv() {
                Ok(Some(dirty)) => Ok(dirty),
                // What a `safe.directory` refusal looks like from here.
                Ok(None) => anyhow::bail!("dubious ownership in repository at {checkout_path}"),
                Err(_) => Ok(false),
            }
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

    /// Wait for the worker threads to have asked.
    fn until_asked(git: &FakeGit, count: usize) {
        until(
            &format!("expected {count} checkouts to be asked about"),
            || git.outstanding() >= count,
        );
    }

    /// Drain until nothing is outstanding.
    fn until_answered(dirty: &mut Dirty) {
        until("answers never arrived", || {
            dirty.drain();
            !dirty.is_waiting()
        });
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

        dirty.reask(&tree);
        until_asked(&git, 2);

        // The stale answer lands first, and says what was true before the reload. Waiting
        // for it to have been taken in — a finished thread pays its slot back — rather than
        // for the picker to settle, which it cannot until the fresh answer arrives too.
        git.answer("/wt/a", true);
        until("the stale answer never arrived", || {
            dirty.drain();
            dirty.in_flight == 1
        });
        assert!(
            dirty_paths(&dirty).is_empty(),
            "the answer belonged to a question that was withdrawn"
        );

        git.answer("/wt/a", false);
        until_answered(&mut dirty);
        assert!(
            dirty_paths(&dirty).is_empty(),
            "and the fresh answer is clean"
        );
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
        assert_eq!(dirty_paths(&dirty), vec!["/wt/a".to_string()]);

        dirty.reask(&tree);
        until_asked(&git, 1);
        git.answer("/wt/a", false);
        until_answered(&mut dirty);
        assert!(dirty_paths(&dirty).is_empty());
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
        assert_eq!(dirty_paths(&dirty), vec!["/wt/a".to_string()]);
    }

    #[test]
    fn a_checkout_git_would_not_answer_for_is_not_recorded_as_clean() {
        // The failure this is really about is the correlated one: `safe.directory`, or a
        // `git` that is not on the path herdr launched the plugin with, refuses every
        // checkout at once. An unmarked list would then be a confident claim that nothing
        // anywhere is holding uncommitted work.
        let git = FakeGit::new();
        let mut dirty = Dirty::new(git.clone());
        let tree = tree(&["/wt/a", "/wt/b"]);

        dirty.ask(&tree);
        until_asked(&git, 2);
        git.refuse("/wt/a");
        git.answer("/wt/b", false);
        until_answered(&mut dirty);

        assert_eq!(
            answered(&dirty),
            vec!["/wt/a".to_string(), "/wt/b".to_string()],
            "both have been asked and both have answered"
        );
        assert!(dirty_paths(&dirty).is_empty(), "neither is marked dirty");
        assert_eq!(
            unreadable(&dirty),
            vec!["/wt/a".to_string()],
            "but only one of them is being called clean"
        );
    }

    #[test]
    fn only_an_answer_that_changes_what_a_row_draws_asks_for_a_redraw() {
        // Most checkouts are clean, and a checkout nobody had asked about turning out to be
        // clean draws exactly what it drew before: nothing. Rebuilding the list for those
        // is work for a list that comes out identical.
        let git = FakeGit::new();
        let mut dirty = Dirty::new(git.clone());
        let tree = tree(&["/wt/a", "/wt/b", "/wt/c"]);
        dirty.ask(&tree);
        until_asked(&git, 3);
        assert!(
            answered(&dirty).is_empty(),
            "asked is not answered, and only the second is a licence to act"
        );

        git.answer("/wt/a", false);
        until("the clean answer never arrived", || {
            dirty.drain();
            dirty.in_flight == 2
        });

        git.answer("/wt/b", true);
        until("the dirty answer never arrived", || {
            dirty.drain();
            dirty.answers().contains_key("/wt/b")
        });

        git.refuse("/wt/c");
        until("the refusal never arrived", || {
            dirty.drain();
            dirty.answers().contains_key("/wt/c")
        });
    }

    #[test]
    fn a_refusal_from_a_withdrawn_round_does_not_keep_a_row_marked() {
        // The scenario the whole state exists for: git is misconfigured, every checkout
        // refuses, the user fixes it and presses `r`. The refusals still in flight are
        // answers to a question that has been withdrawn.
        let git = FakeGit::new();
        let mut dirty = Dirty::new(git.clone());
        let tree = tree(&["/wt/a"]);

        dirty.ask(&tree);
        until_asked(&git, 1);
        dirty.reask(&tree);
        until_asked(&git, 2);

        git.refuse("/wt/a");
        until("the stale refusal never arrived", || {
            dirty.drain();
            dirty.in_flight == 1
        });
        assert!(unreadable(&dirty).is_empty(), "it was withdrawn");

        git.answer("/wt/a", false);
        until_answered(&mut dirty);
        assert!(unreadable(&dirty).is_empty());
        assert!(dirty_paths(&dirty).is_empty());
    }

    #[test]
    fn a_checkout_that_has_left_the_tree_is_forgotten() {
        // Kept for the life of the picker, so without this a session's worth of deleted
        // checkouts accumulates — and every one is an answer about a working tree that is
        // no longer there.
        let git = FakeGit::new();
        let mut dirty = Dirty::new(git.clone());

        dirty.ask(&tree(&["/wt/a", "/wt/b"]));
        until_asked(&git, 2);
        git.refuse("/wt/a");
        git.answer("/wt/b", true);
        until_answered(&mut dirty);
        assert_eq!(unreadable(&dirty), vec!["/wt/a".to_string()]);
        assert_eq!(dirty_paths(&dirty), vec!["/wt/b".to_string()]);

        // `/wt/a` is deleted; the picker collects the tree again and asks about what is
        // left.
        dirty.ask(&tree(&["/wt/b"]));
        assert!(unreadable(&dirty).is_empty());
        assert_eq!(dirty_paths(&dirty), vec!["/wt/b".to_string()]);
        assert_eq!(git.outstanding(), 0, "and nothing is asked twice");
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
        // `pump` spawns on this thread and nothing has drained, so this is the whole
        // population rather than a snapshot of a moving one.
        assert_eq!(git.outstanding(), MAX_IN_FLIGHT);

        for path in paths.iter().take(MAX_IN_FLIGHT) {
            git.answer(path, false);
        }

        // The next slots open on the drain, which is where the loop takes answers in.
        until("slots never came free", || {
            dirty.drain();
            git.outstanding() >= MAX_IN_FLIGHT
        });
        assert_eq!(
            git.outstanding(),
            MAX_IN_FLIGHT,
            "the rest follow as slots come free, and no faster"
        );
        assert!(dirty.is_waiting());
    }
}
