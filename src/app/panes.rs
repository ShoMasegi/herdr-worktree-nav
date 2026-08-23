//! The panes picker: draw, read a key, and act on what it meant.

use anyhow::Result;
use ratatui::crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::app::collect;
use crate::app::home_dir;
use crate::port::{GitPort, HerdrPort, PaneSplit, SplitDirection, WorktreeOpen};
use crate::ui::render::{self, Mode};
use crate::ui::state::{Action, PanesState};
use crate::ui::theme::Theme;

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
    initial_pane: Option<&str>,
    theme: &Theme,
) -> Result<Exit> {
    let (_, tree) = collect::collect_tree(herdr, git)?;
    let mut state = PanesState::new(tree, home_dir());
    if let Some(pane_id) = initial_pane {
        state.focus_pane(pane_id);
    }

    let outcome = loop {
        terminal.draw(|frame| render::draw(frame, &state, theme, Mode::Panes))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        match state.handle_key(key) {
            Action::Consumed | Action::Ignored => {}
            Action::Reload => {
                // Errors here are not fatal: the picker keeps showing what it had.
                if let Ok((_, tree)) = collect::collect_tree(herdr, git) {
                    state.replace_tree(tree);
                }
            }
            // Deleting is housekeeping, and housekeeping comes in batches: the picker stays
            // open on the list the deletion just changed rather than closing over it.
            Action::RemoveWorktree {
                repo_root,
                checkout_path,
                label,
            } => match git.remove_worktree(&repo_root, &checkout_path) {
                Ok(()) => {
                    state.set_message(format!("removed the checkout for {label}"));
                    if let Ok((_, tree)) = collect::collect_tree(herdr, git) {
                        state.replace_tree(tree);
                    }
                }
                // git refuses a checkout with work in it, which is the answer rather than
                // an obstacle: it says what would have been lost.
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
