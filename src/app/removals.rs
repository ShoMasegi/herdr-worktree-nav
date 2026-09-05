//! Removals the picker has started and no longer owns.
//!
//! Each one is a process of its own that finishes whether or not anything here is still
//! watching, and reports itself to herdr either way — see
//! `docs/adr/0014-removing-outlives-the-picker.md`. What is kept here is only what the
//! picker needs while it happens to still be up: which rows are going, and what to say
//! about the ones that come back refused.

use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::Result;

use crate::domain::removal::{self, Removal};
use crate::port::{HerdrPort, RemovalOutcome, RemovalPort};

/// One removal that has been started and has not reported back.
struct InFlight {
    checkout_path: String,
    /// The branch, which is what a refusal names.
    label: String,
    /// How many panes were stopped before this started, which a refusal has to mention.
    panes_closed: usize,
}

/// A removal that has reported back.
pub struct Finished {
    pub label: String,
    /// What was stopped to get here. A refusal after this is not "nothing happened".
    pub panes_closed: usize,
    /// `Err` when the process ended without saying anything readable. The toast, if there
    /// was one, has already been shown by then — this is only what this side knows.
    pub outcome: Result<RemovalOutcome>,
}

/// The removals started from this picker.
///
/// Owned by the view switch rather than by the panes view, for the same reason the listing
/// cache is (`docs/adr/0009-the-picker-owns-the-terminal.md`): `Tab` away and back is one
/// frame of the same picker, and a view that forgot what was going would draw a finished
/// row as though nothing were happening to it — and let a second `Shift-D` reach it.
pub struct Removals<'a> {
    port: &'a dyn RemovalPort,
    sender: Sender<(String, Result<RemovalOutcome>)>,
    receiver: Receiver<(String, Result<RemovalOutcome>)>,
    in_flight: Vec<InFlight>,
}

impl<'a> Removals<'a> {
    pub fn new(port: &'a dyn RemovalPort) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            port,
            sender,
            receiver,
            in_flight: Vec::new(),
        }
    }

    /// Close a checkout's panes and then start removing it, stopping at the first thing that
    /// fails. `Err` is what to tell the user, already in words.
    ///
    /// This is the only way in, and that is the point. `git worktree remove` walking a
    /// working tree that still has agents writing into it is what
    /// `docs/adr/0010-closing-the-panes-first.md` is about, and an order that holds only
    /// because every caller remembered it is one the next caller breaks in silence. The
    /// panes come off the `Removal` rather than from an argument, so there is no second list
    /// to pair with the wrong checkout or to leave empty for a checkout that has panes.
    ///
    /// They are closed here rather than in the process that carries out the removal because
    /// by the time that runs they are gone: the grouping it could rebuild for itself would
    /// be a grouping with nothing in it. herdr collapses a tab and a workspace that end up
    /// empty, which is what lets this leave no residue — measured against 0.7.4.
    ///
    /// A pane that will not close stops the whole thing: a checkout removed out from under
    /// half its panes is worse than one not removed. How far it got is what the message is
    /// for — the panes that did close are gone, and nothing else on screen will say so.
    pub fn remove(&mut self, herdr: &dyn HerdrPort, removal: &Removal) -> Result<(), String> {
        self.close_panes(herdr, removal)?;
        // Every pane is gone by now, so a failure here is the same shape as a git refusal
        // and gets the same clause: the worst version of it, in fact, since none survived.
        self.start(removal).map_err(|error| {
            format!(
                "could not start removing {}: {}",
                removal.label,
                removal::refusal(&format!("{error:#}"), removal.panes().len())
            )
        })
    }

    /// Close every pane the removal names, in the order it lists them, stopping at the first
    /// that refuses. How far it got is in the message, because the panes before the refusal
    /// are gone and nothing else on screen says so.
    ///
    /// A pane that has already gone is not a refusal, but that is not decided here: see
    /// [`HerdrPort::pane_close`], which is where a pane herdr no longer knows about becomes
    /// an `Ok`. Anything that does reach here as an `Err` stops the walk.
    fn close_panes(&self, herdr: &dyn HerdrPort, removal: &Removal) -> Result<(), String> {
        let total = removal.panes().len();
        for (closed, pane_id) in removal.pane_ids().iter().enumerate() {
            if let Err(error) = herdr.pane_close(pane_id) {
                return Err(removal::interrupted(
                    pane_id,
                    &format!("{error:#}"),
                    closed,
                    total,
                ));
            }
        }
        Ok(())
    }

    /// Start the removal process. Returns once it is running, which is the point: the wait
    /// happens on a thread of its own so the picker keeps drawing and keeps reading keys.
    ///
    /// Private, and that is the whole enforcement of ADR 0010's ordering — there is no
    /// argument to forge and no second entry point to forget the close.
    fn start(&mut self, removal: &Removal) -> Result<()> {
        let Removal {
            repo_root,
            checkout_path,
            label,
            ..
        } = removal;
        let panes_closed = removal.panes().len();
        let running = self
            .port
            .start(repo_root, checkout_path, label, panes_closed)?;
        let sender = self.sender.clone();
        let path = checkout_path.to_string();
        // Not joined anywhere. Leaving the picker ends this thread with the process, and the
        // removal it was waiting on carries on without either of them.
        std::thread::spawn(move || {
            let outcome = running.wait();
            let _ = sender.send((path, outcome));
        });
        self.in_flight.push(InFlight {
            checkout_path: checkout_path.to_string(),
            label: label.to_string(),
            panes_closed,
        });
        Ok(())
    }

    /// The checkouts currently going, for the rows that stand for them.
    pub fn paths(&self) -> Vec<String> {
        self.in_flight
            .iter()
            .map(|removal| removal.checkout_path.clone())
            .collect()
    }

    /// Whether anything is running. The panes loop polls on a clock only while this is
    /// false — with nothing to wait for, blocking on a key draws no frames at all.
    pub fn is_empty(&self) -> bool {
        self.in_flight.is_empty()
    }

    /// The next removal to have reported back, if any. Never blocks.
    pub fn finished(&mut self) -> Option<Finished> {
        let (checkout_path, outcome) = self.receiver.try_recv().ok()?;
        let (label, panes_closed) = match self
            .in_flight
            .iter()
            .position(|removal| removal.checkout_path == checkout_path)
        {
            Some(index) => {
                let removal = self.in_flight.remove(index);
                (removal.label, removal.panes_closed)
            }
            // Cannot happen: nothing sends without having been pushed first. The path is a
            // usable name for it either way, and dropping the answer would leave a spinner
            // turning over a removal that has finished.
            None => (checkout_path, 0),
        };
        Some(Finished {
            label,
            panes_closed,
            outcome,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fakes::{until, Recorder, Started};
    use crate::domain::model::{PaneNode, WorktreeNode};
    use crate::port::{AgentStatus, RemovalOutcome, RunningRemoval};
    use std::sync::Mutex;

    /// A removal that finishes when the test says so, so two can be made to answer in the
    /// order the test chooses rather than the order they were started in.
    struct Held(std::sync::mpsc::Receiver<RemovalOutcome>);

    impl RunningRemoval for Held {
        fn wait(self: Box<Self>) -> Result<RemovalOutcome> {
            Ok(self.0.recv()?)
        }
    }

    #[derive(Default)]
    struct FakeRemover {
        /// One sender per started removal, in the order they were started.
        started: Mutex<Vec<(String, std::sync::mpsc::Sender<RemovalOutcome>)>>,
    }

    impl FakeRemover {
        fn finish(&self, checkout_path: &str, outcome: RemovalOutcome) {
            let started = self.started.lock().unwrap();
            let (_, sender) = started
                .iter()
                .find(|(path, _)| path == checkout_path)
                .unwrap_or_else(|| panic!("nothing was started for {checkout_path}"));
            sender
                .send(outcome)
                .expect("the waiter should still be there");
        }
    }

    impl RemovalPort for FakeRemover {
        fn start(
            &self,
            _repo_root: &str,
            checkout_path: &str,
            _label: &str,
            _panes_closed: usize,
        ) -> Result<Box<dyn RunningRemoval>> {
            let (sender, receiver) = mpsc::channel();
            self.started
                .lock()
                .unwrap()
                .push((checkout_path.to_string(), sender));
            Ok(Box::new(Held(receiver)))
        }
    }

    /// A checkout to remove, named by the panes in it. Built the only way one can be —
    /// through the checkout — so the pane list is the checkout's own. Nothing here reads a
    /// pane beyond its id, so the rest is whatever a tree would have put there.
    fn removal(checkout_path: &str, label: &str, panes: &[&str]) -> Removal {
        Removal::of(
            "/src/app",
            &WorktreeNode {
                branch: Some(label.to_string()),
                checkout_path: checkout_path.to_string(),
                is_primary: false,
                open_workspace_id: None,
                track: None,
                panes: panes
                    .iter()
                    .map(|pane_id| PaneNode {
                        pane_id: (*pane_id).to_string(),
                        workspace_id: "w1".into(),
                        tab_id: "t1".into(),
                        display_name: None,
                        agent_status: AgentStatus::default(),
                        focused: false,
                    })
                    .collect(),
            },
        )
    }

    #[test]
    fn every_pane_closes_before_the_removal_starts_and_in_the_order_given() {
        // The order is the safety rule: `git worktree remove` walking a working tree that
        // still has agents writing into it is what closing first exists to prevent. Both
        // fakes write into one log, so the rule is one sequence rather than two.
        let recorder = Recorder::default();
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);

        let told = removals.remove(
            &recorder,
            &removal("/wt/feat-login", "feat/login", &["w2:p1", "w2:p2", "w9:p3"]),
        );

        assert_eq!(told, Ok(()));
        assert_eq!(
            recorder.did(),
            [
                "close w2:p1",
                "close w2:p2",
                "close w9:p3",
                "start /wt/feat-login after 3",
            ]
        );
    }

    #[test]
    fn a_pane_that_will_not_close_stops_the_removal_and_says_how_far_it_got() {
        // Half the panes gone and the checkout still standing is not "nothing happened",
        // and herdr's bare refusal does not say which of the two the reader is looking at.
        let recorder = Recorder::refusing("w2:p2");
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);

        let told = removals.remove(
            &recorder,
            &removal("/wt/feat-login", "feat/login", &["w2:p1", "w2:p2", "w9:p3"]),
        );

        assert_eq!(
            told,
            Err(
                "could not close w2:p2: herdr rejected pane.close: no such pane (not_found) \
                 — 1 of its 3 panes was closed first, and the checkout was not removed"
                    .to_string()
            )
        );
        assert_eq!(
            recorder.did(),
            ["close w2:p1"],
            "it stopped there, and started nothing"
        );
        assert!(removals.is_empty());
    }

    #[test]
    fn a_checkout_with_no_panes_starts_its_removal_straight_away() {
        let recorder = Recorder::default();
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);

        assert_eq!(
            removals.remove(&recorder, &removal("/wt/fix-crash", "fix/crash", &[])),
            Ok(())
        );
        assert_eq!(recorder.did(), ["start /wt/fix-crash after 0"]);
    }

    #[test]
    fn an_answer_is_matched_to_the_removal_that_asked_for_it() {
        // Several can be in flight at once, and they do not answer in the order they were
        // started. Getting this wrong names the wrong branch in the refusal and miscounts
        // the panes it closed — both of which read as facts about the wrong checkout.
        let port = FakeRemover::default();
        let herdr = Recorder::default();
        let mut removals = Removals::new(&port);
        removals
            .remove(
                &herdr,
                &removal("/wt/a", "feat/a", &["w1:p1", "w1:p2", "w1:p3"]),
            )
            .unwrap();
        removals
            .remove(&herdr, &removal("/wt/b", "feat/b", &["w2:p1"]))
            .unwrap();
        assert_eq!(removals.paths(), ["/wt/a".to_string(), "/wt/b".to_string()]);

        // The second one answers first.
        port.finish("/wt/b", RemovalOutcome::Refused("no".into()));
        let mut finished = None;
        until("the second removal never reported", || {
            finished = removals.finished();
            finished.is_some()
        });
        let finished = finished.unwrap();
        assert_eq!(finished.label, "feat/b");
        assert_eq!(finished.panes_closed, 1);
        assert_eq!(
            removals.paths(),
            ["/wt/a".to_string()],
            "the other is still going"
        );

        port.finish("/wt/a", RemovalOutcome::Removed);
        let mut finished = None;
        until("the first removal never reported", || {
            finished = removals.finished();
            finished.is_some()
        });
        let finished = finished.unwrap();
        assert_eq!(finished.label, "feat/a");
        assert_eq!(finished.panes_closed, 3);
        assert!(removals.is_empty());
    }
}
