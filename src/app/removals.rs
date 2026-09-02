//! Removals the picker has started and no longer owns.
//!
//! Each one is a process of its own that finishes whether or not anything here is still
//! watching, and reports itself to herdr either way — see
//! `docs/adr/0014-removing-outlives-the-picker.md`. What is kept here is only what the
//! picker needs while it happens to still be up: which rows are going, and what to say
//! about the ones that come back refused.

use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::Result;

use crate::port::{RemovalOutcome, RemovalPort};

/// One removal that has been started and has not reported back.
struct InFlight {
    checkout_path: String,
    /// The branch, which is what a refusal names.
    label: String,
}

/// A removal that has reported back.
pub struct Finished {
    pub label: String,
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
    pub fn start(&mut self, repo_root: &str, checkout_path: &str, label: &str) -> Result<()> {
        let running = self.port.start(repo_root, checkout_path, label)?;
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
        let label = match self
            .in_flight
            .iter()
            .position(|removal| removal.checkout_path == checkout_path)
        {
            Some(index) => self.in_flight.remove(index).label,
            // Cannot happen: nothing sends without having been pushed first. The path is a
            // usable name for it either way, and dropping the answer would leave a spinner
            // turning over a removal that has finished.
            None => checkout_path,
        };
        Some(Finished { label, outcome })
    }
}
