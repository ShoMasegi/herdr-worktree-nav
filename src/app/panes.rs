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
        }
        let reading_working_trees = dirty.is_waiting();
        state.set_waiting(reading_working_trees);
        state.set_unreadable(dirty.unreadable());
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
                    if let Some(message) = removal::message(&finished.label, &outcome) {
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
                    dirty.forget();
                    dirty.ask(state.tree());
                    state.set_dirty(dirty.paths());
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
            } => match removals.start(&repo_root, &checkout_path, &label) {
                // The row says what is happening to it; there is nothing to add here.
                Ok(()) => state.set_removing(removals.paths()),
                Err(error) => state.set_message(format!("{error:#}")),
            },
            action => break action,
        }
    };

    perform(herdr, outcome)
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
