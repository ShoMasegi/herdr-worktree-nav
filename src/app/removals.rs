//! Removals the picker has started and no longer owns.
//!
//! Each one is a process of its own that finishes whether or not anything here is still
//! watching, and reports itself to herdr either way — see
//! `docs/adr/0014-removing-outlives-the-picker.md`. What is kept here is only what the
//! picker needs while it happens to still be up: which rows are going, and what to say
//! about the ones that come back refused.

use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::Result;

use crate::domain::removal;
use crate::port::{HerdrPort, RemovalOutcome, RemovalPort};

/// Panes this process has closed, just now, for the removal about to be started.
///
/// The field is private and [`Removals::close_panes`] is the only thing that makes one, so
/// a removal cannot be started with a count it did not earn — and, more to the point, cannot
/// be started before the panes are closed at all, because the argument does not exist yet.
/// `git worktree remove` walking a working tree that still has agents writing into it is
/// what `docs/adr/0010-closing-the-panes-first.md` is about, and until now the only thing
/// standing between that and a second caller was one private function.
#[derive(Debug)]
pub struct PanesClosed(usize);

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

    /// Start removing a checkout. Returns once the process is running, which is the point:
    /// the wait happens on a thread of its own so the picker keeps drawing and keeps
    /// reading keys.
    /// Close every pane in a checkout, in the order given, stopping at the first that
    /// refuses. `Err` is what to tell the user, already in words: how far it got matters,
    /// because the panes before the refusal are gone and nothing else on screen says so.
    ///
    /// A pane that has already gone is not a refusal — it is the state this asks for, and it
    /// happens on its own whenever a pane's command finishes.
    pub fn close_panes(
        &self,
        herdr: &dyn HerdrPort,
        panes: &[String],
    ) -> Result<PanesClosed, String> {
        for (closed, pane_id) in panes.iter().enumerate() {
            if let Err(error) = herdr.pane_close(pane_id) {
                return Err(removal::interrupted(
                    pane_id,
                    &format!("{error:#}"),
                    closed,
                    panes.len(),
                ));
            }
        }
        Ok(PanesClosed(panes.len()))
    }

    pub fn start(
        &mut self,
        repo_root: &str,
        checkout_path: &str,
        label: &str,
        closed: PanesClosed,
    ) -> Result<()> {
        let panes_closed = closed.0;
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
    use crate::app::fakes::{until, Recorder};
    use crate::port::{RemovalOutcome, RunningRemoval};
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

    fn ids(panes: &[&str]) -> Vec<String> {
        panes.iter().map(|id| (*id).to_string()).collect()
    }

    #[test]
    fn a_removal_cannot_be_started_without_having_closed_the_panes_first() {
        // Not an assertion so much as a note on what the type does: `start` takes a
        // `PanesClosed`, and `close_panes` is the only thing that makes one. Writing
        // `removals.start(.., 0)` — the shape a second caller would reach for — does not
        // compile, which is the whole point of the type existing.
        let port = FakeRemover::default();
        let herdr = Recorder::default();
        let mut removals = Removals::new(&port);

        let closed = removals
            .close_panes(&herdr, &ids(&["w1:p1", "w1:p2"]))
            .unwrap();
        assert_eq!(herdr.did(), ["close w1:p1", "close w1:p2"]);
        removals
            .start("/src/app", "/wt/a", "feat/a", closed)
            .unwrap();
    }

    #[test]
    fn a_pane_that_refuses_stops_the_close_and_mints_nothing() {
        // No `PanesClosed` comes back, so there is nothing to start a removal with — the
        // half-dismantled checkout cannot be removed even by a caller that ignores the
        // error.
        let port = FakeRemover::default();
        let herdr = Recorder::refusing("w1:p2");
        let removals = Removals::new(&port);

        let told = removals.close_panes(&herdr, &ids(&["w1:p1", "w1:p2", "w1:p3"]));
        assert_eq!(
            told.unwrap_err(),
            "could not close w1:p2: herdr rejected pane.close: no such pane (not_found) \
             — 1 of its 3 panes was closed first, and the checkout was not removed"
        );
        assert_eq!(herdr.did(), ["close w1:p1"], "it stopped there");
    }

    #[test]
    fn an_answer_is_matched_to_the_removal_that_asked_for_it() {
        // Several can be in flight at once, and they do not answer in the order they were
        // started. Getting this wrong names the wrong branch in the refusal and miscounts
        // the panes it closed — both of which read as facts about the wrong checkout.
        let port = FakeRemover::default();
        let herdr = Recorder::default();
        let mut removals = Removals::new(&port);
        let three = removals
            .close_panes(&herdr, &ids(&["w1:p1", "w1:p2", "w1:p3"]))
            .unwrap();
        removals
            .start("/src/app", "/wt/a", "feat/a", three)
            .unwrap();
        let one = removals.close_panes(&herdr, &ids(&["w2:p1"])).unwrap();
        removals.start("/src/app", "/wt/b", "feat/b", one).unwrap();
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
