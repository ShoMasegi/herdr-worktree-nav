//! The panes picker: draw, read a key, and act on what it meant.

use anyhow::Result;
use ratatui::crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::app::collect;
use crate::app::dirty::Dirty;
use crate::app::home_dir;
use crate::app::removals::Removals;
use crate::domain::removal;
use crate::port::{GitPort, HerdrPort, PaneSplit, SplitDirection, WorktreeOpen};
use crate::ui::render::{self, Mode};
use crate::ui::state::{Action, PanesState};
use crate::ui::theme::Theme;

/// How long to wait for a key before turning the spinner on whatever is still coming. The
/// same tick the branches view runs on; with nothing outstanding this loop does not use one.
const TICK: std::time::Duration = std::time::Duration::from_millis(80);

/// What the picker was left wanting when it closed. The caller decides whether that means
/// switching views or exiting.
pub enum Exit {
    Closed,
    /// `None` when the cursor was not in a repository: the branches picker opens on its
    /// repository list either way.
    ShowBranches {
        repo_root: Option<String>,
    },
}

/// Run the picker to completion on the terminal the picker already holds. `run_picker` puts
/// it back on every path out, so a failure still surfaces as text rather than as a corrupted
/// screen — it is simply printed after the picker has finished rather than before.
pub fn run(
    terminal: &mut DefaultTerminal,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    removals: &mut Removals,
    dirty: &mut Dirty,
    initial_pane: Option<&str>,
    theme: &Theme,
) -> Result<Exit> {
    let (_, tree) = collect::collect_tree(herdr, git)?;
    let mut state = PanesState::new(tree, home_dir());
    // Both outlive this view: a removal started before a trip through the branches view is
    // still going, and a working tree walked once does not need walking again.
    dirty.ask(state.tree());
    state.set_dirty(dirty.paths());
    state.set_unreadable(dirty.unreadable());
    state.set_answered(dirty.answered());
    state.set_removing(removals.paths());
    if let Some(pane_id) = initial_pane {
        state.focus_pane(pane_id);
    }

    // The spinner runs on a clock rather than on redraws, so it neither speeds up while the
    // user types nor stalls while they hold a key down.
    let mut last_tick = std::time::Instant::now();
    let outcome = loop {
        if dirty.drain() {
            state.set_dirty(dirty.paths());
            state.set_unreadable(dirty.unreadable());
        }
        let reading_working_trees = dirty.is_waiting();
        state.set_waiting(reading_working_trees);
        // Every frame, not only when a marker moved: a clean answer moves none, and it is
        // exactly the answer that turns a refusal into an offer.
        state.set_answered(dirty.answered());
        let waiting = !removals.is_empty() || reading_working_trees;
        if waiting && last_tick.elapsed() >= TICK {
            state.tick();
            last_tick = std::time::Instant::now();
        }
        terminal.draw(|frame| render::draw(frame, &state, theme, Mode::Panes))?;

        // Whatever has reported back — including from before the last trip to the branches
        // view, since the removals outlive both views and the picker itself.
        while let Some(finished) = removals.finished() {
            state.set_removing(removals.paths());
            match finished.outcome {
                Ok(outcome) => {
                    // Nothing to say when it worked: the row leaving the list is the report,
                    // and the toast has already said it to whoever was not looking.
                    if let Some(message) =
                        removal::message(&finished.label, &outcome, finished.panes_closed)
                    {
                        state.set_message(message);
                    }
                    // Errors here are not fatal: the picker keeps showing what it had.
                    if let Ok((_, tree)) = collect::collect_tree(herdr, git) {
                        state.replace_tree(tree);
                        dirty.ask(state.tree());
                    }
                }
                Err(error) => state.set_message(format!("{error:#}")),
            }
        }

        // With nothing in flight there is nothing to wake up for, so the loop blocks on the
        // key and draws no frames at all until one arrives.
        if waiting && !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        match state.handle_key(key) {
            Action::Consumed | Action::Ignored => {}
            // `r` is the only thing that asks about the working trees again, so a reload
            // that quietly does nothing is a reload the user reads as "still dirty, then".
            Action::Reload => match collect::collect_tree(herdr, git) {
                Ok((_, tree)) => {
                    state.replace_tree(tree);
                    // Reload means reload: whether a checkout is dirty is a fact about a
                    // working tree the user has been editing since it was last asked.
                    dirty.reask(state.tree());
                    state.set_dirty(dirty.paths());
                    state.set_unreadable(dirty.unreadable());
                }
                Err(error) => state.set_message(format!("{error:#}")),
            },
            // Deleting is housekeeping, and housekeeping comes in batches: the picker stays
            // open on the list the deletion is changing rather than closing over it — and
            // the deletion itself goes to a process of its own, so that neither the loop
            // nor the user has to wait for git to walk a working tree. See
            // `docs/adr/0014-removing-outlives-the-picker.md`.
            Action::RemoveWorktree {
                repo_root,
                checkout_path,
                label,
                panes,
            } => {
                match close_then_remove(herdr, removals, &repo_root, &checkout_path, &label, &panes)
                {
                    // The row says what is happening to it; nothing to add here.
                    Ok(()) => state.set_removing(removals.paths()),
                    Err(message) => state.set_message(message),
                }
                // Only when panes went. The list is then known wrong rather than merely
                // possibly stale, and an empty checkout's removal leaves the cursor where it
                // was — which is where the next thing to tidy up usually is.
                if !panes.is_empty() {
                    match collect::collect_tree(herdr, git) {
                        Ok((_, tree)) => {
                            state.replace_tree(tree);
                            dirty.ask(state.tree());
                            state.set_answered(dirty.answered());
                        }
                        // Not fatal, but not silent either: rows for panes that have
                        // certainly stopped are still on screen until this succeeds.
                        Err(error) => state.set_message(format!(
                            "the panes closed, but the list could not be read again: {error:#}"
                        )),
                    }
                }
            }
            action => break action,
        }
    };

    perform(herdr, outcome)
}

/// Close every pane in a checkout and then start removing it, stopping at the first thing
/// that fails. `Err` is what to tell the user, already in words.
///
/// The panes are closed here rather than in the process that carries out the removal because
/// by the time that runs they are gone: the grouping it could rebuild for itself would be a
/// grouping with nothing in it. herdr collapses a tab and a workspace that end up empty,
/// which is what lets this leave no residue — measured against 0.7.4. See
/// `docs/adr/0010-closing-the-panes-first.md`.
///
/// A pane that will not close stops the whole thing: a checkout removed out from under half
/// its panes is worse than one not removed. How far it got is what the message is for, since
/// the tab those panes were in has collapsed and there is nothing left on screen to say why
/// somebody's work stopped.
fn close_then_remove(
    herdr: &dyn HerdrPort,
    removals: &mut Removals,
    repo_root: &str,
    checkout_path: &str,
    label: &str,
    panes: &[String],
) -> Result<(), String> {
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
    removals
        .start(repo_root, checkout_path, label, panes.len())
        .map_err(|error| format!("{error:#}"))
}

fn perform(herdr: &dyn HerdrPort, action: Action) -> Result<Exit> {
    match action {
        Action::Quit => Ok(Exit::Closed),
        // Focus, then exit. herdr tears the overlay down once this process ends, and the
        // focus set just before that is what the user is left looking at.
        Action::Jump(pane_id) => {
            herdr.pane_focus(&pane_id)?;
            Ok(Exit::Closed)
        }
        Action::OpenWorktree {
            repo_root,
            checkout_path,
        } => {
            herdr.worktree_open(&WorktreeOpen {
                cwd: repo_root,
                path: Some(checkout_path),
                branch: None,
                focus: true,
            })?;
            Ok(Exit::Closed)
        }
        Action::NewPane {
            checkout_path,
            beside_pane_id,
        } => {
            herdr.pane_split(&PaneSplit {
                target_pane_id: beside_pane_id,
                direction: SplitDirection::Right,
                cwd: Some(checkout_path),
                focus: true,
            })?;
            Ok(Exit::Closed)
        }
        Action::ShowBranches { repo_root } => Ok(Exit::ShowBranches { repo_root }),
        // Handled inside the loop, which is why the picker is still up after one.
        Action::Consumed | Action::Ignored | Action::Reload | Action::RemoveWorktree { .. } => {
            Ok(Exit::Closed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{
        Notification, Pane, PaneDestination, PluginPaneOpen, RemovalOutcome, RemovalPort,
        RunningRemoval, Snapshot, WorktreeCreate, WorktreeList, WorktreeOpened,
    };
    use anyhow::{anyhow, Result};
    use std::sync::Mutex;

    /// Records the order it was asked to do things in, and can be told to refuse one pane.
    /// The ordering is the whole of `docs/adr/0010-closing-the-panes-first.md` and there is
    /// nowhere else it can be checked: the picker's loop needs a terminal and a keyboard.
    #[derive(Default)]
    struct Recorder {
        did: Mutex<Vec<String>>,
        refuse: Option<String>,
    }

    impl Recorder {
        fn refusing(pane_id: &str) -> Self {
            Self {
                refuse: Some(pane_id.to_string()),
                ..Self::default()
            }
        }

        fn did(&self) -> Vec<String> {
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
            self.did.lock().unwrap().push(format!("close {pane_id}"));
            Ok(())
        }

        fn snapshot(&self) -> Result<Snapshot> {
            unreachable!("only pane_close is asked of this port")
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
        fn pane_move(&self, _p: &str, _d: &PaneDestination, _f: bool) -> Result<()> {
            unreachable!()
        }
        fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
            unreachable!()
        }
        fn tab_focus(&self, _tab_id: &str) -> Result<()> {
            unreachable!()
        }
        fn plugin_pane_open(
            &self,
            _req: &PluginPaneOpen,
        ) -> Result<Option<crate::port::OpenRefusal>> {
            unreachable!()
        }
        fn notify(&self, _notification: &Notification) -> Result<()> {
            unreachable!()
        }
    }

    /// Starts nothing; it only says that it was asked, into the same log the closes go to.
    struct Started<'a>(&'a Recorder);

    struct Done;

    impl RunningRemoval for Done {
        fn wait(self: Box<Self>) -> Result<RemovalOutcome> {
            Ok(RemovalOutcome::Removed)
        }
    }

    impl RemovalPort for Started<'_> {
        fn start(
            &self,
            _repo_root: &str,
            checkout_path: &str,
            _label: &str,
            panes_closed: usize,
        ) -> Result<Box<dyn RunningRemoval>> {
            self.0
                .did
                .lock()
                .unwrap()
                .push(format!("start {checkout_path} after {panes_closed}"));
            Ok(Box::new(Done))
        }
    }

    fn ids(panes: &[&str]) -> Vec<String> {
        panes.iter().map(|id| (*id).to_string()).collect()
    }

    #[test]
    fn every_pane_closes_before_the_removal_starts_and_in_the_order_given() {
        // The order is the safety rule: `git worktree remove` walking a working tree that
        // still has agents writing into it is what closing first exists to prevent.
        let recorder = Recorder::default();
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);

        let told = close_then_remove(
            &recorder,
            &mut removals,
            "/src/app",
            "/wt/feat-login",
            "feat/login",
            &ids(&["w2:p1", "w2:p2", "w9:p3"]),
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

        let told = close_then_remove(
            &recorder,
            &mut removals,
            "/src/app",
            "/wt/feat-login",
            "feat/login",
            &ids(&["w2:p1", "w2:p2", "w9:p3"]),
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
            close_then_remove(
                &recorder,
                &mut removals,
                "/src/app",
                "/wt/fix-crash",
                "fix/crash",
                &[],
            ),
            Ok(())
        );
        assert_eq!(recorder.did(), ["start /wt/fix-crash after 0"]);
    }
}
