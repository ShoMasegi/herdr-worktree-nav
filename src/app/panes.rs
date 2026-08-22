//! The panes picker: draw, read a key, and act on what it meant.

use anyhow::Result;
use ratatui::crossterm::event::{self, Event};

use crate::app::collect;
use crate::port::{GitPort, HerdrPort, PaneSplit, SplitDirection, WorktreeOpen};
use crate::ui::render::{self, Mode};
use crate::ui::state::{Action, PanesState};

/// What the picker was left wanting when it closed. The caller decides whether that means
/// switching views or exiting.
pub enum Exit {
    Closed,
    ShowBranches { repo_root: String },
}

/// Run the picker to completion. The terminal is restored before any herdr call, so a
/// failure surfaces as text rather than as a corrupted screen.
pub fn run(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    initial_pane: Option<&str>,
    own_pane_id: Option<&str>,
) -> Result<Exit> {
    let (_, tree) = collect::collect_tree(herdr, git, own_pane_id)?;
    let mut state = PanesState::new(tree);
    if let Some(pane_id) = initial_pane {
        state.focus_pane(pane_id);
    }

    let mut terminal = ratatui::try_init()?;
    let outcome = loop {
        terminal.draw(|frame| render::draw(frame, &state, Mode::Panes))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        match state.handle_key(key) {
            Action::Consumed | Action::Ignored => {}
            Action::Reload => {
                // Errors here are not fatal: the picker keeps showing what it had.
                if let Ok((_, tree)) = collect::collect_tree(herdr, git, own_pane_id) {
                    state.replace_tree(tree);
                }
            }
            action => break action,
        }
    };
    ratatui::try_restore()?;

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
        Action::Consumed | Action::Ignored | Action::Reload => Ok(Exit::Closed),
    }
}
