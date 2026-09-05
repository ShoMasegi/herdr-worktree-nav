//! The panes picker: draw, read a key, and act on what it meant.

use anyhow::Result;
use ratatui::crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::app::collect;
use crate::app::dirty::Dirty;
use crate::app::home_dir;
use crate::app::removals::Removals;
use crate::domain::removal::{self, Removal};
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
    // still going, and a working tree walked once does not need walking again. These two
    // have to be seeded because `show_answers` only refreshes them when `drain` reports a
    // change, and coming back to a view where nothing has moved reports none. `set_answered`
    // and `set_waiting` need no seeding — `show_answers` sets those every frame, and the
    // first one runs before the first draw.
    dirty.ask(state.tree());
    state.set_dirty(dirty.paths());
    state.set_unreadable(dirty.unreadable());
    state.set_removing(removals.paths());
    if let Some(pane_id) = initial_pane {
        state.focus_pane(pane_id);
    }

    // The spinner runs on a clock rather than on redraws, so it neither speeds up while the
    // user types nor stalls while they hold a key down.
    let mut last_tick = std::time::Instant::now();
    let outcome = loop {
        let reading_working_trees = show_answers(&mut state, dirty);
        let waiting = !removals.is_empty() || reading_working_trees;
        if waiting && last_tick.elapsed() >= TICK {
            state.tick();
            last_tick = std::time::Instant::now();
        }
        let mut asked = true;
        terminal.draw(|frame| asked = render::draw(frame, &state, theme, Mode::Panes))?;
        if !asked {
            // The pane is too small to put the question in. Taking it back is the only
            // honest answer: the alternative is `y` armed over a box nobody ever saw.
            state.cancel_removal();
            state.set_message("this pane is too small to ask that safely".into());
        }

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
                // The panes are gone by now either way, so the report says so.
                Err(error) => state.set_message(removal::refusal(
                    &format!("{error:#}"),
                    finished.panes_closed,
                )),
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
            Action::RemoveWorktree(removal) => {
                start_removal(&mut state, dirty, removals, herdr, git, &removal)
            }
            action => break action,
        }
    };

    perform(herdr, outcome)
}

/// Tell the state what the working-tree walk has said since the last frame, and answer
/// whether any of it is still coming.
///
/// Split out of the loop because everything the loop does is otherwise untestable — it needs
/// a terminal and a keyboard — and this is the part with consequences. `set_answered` in
/// particular has to run every frame and not only when a marker moved: a clean answer moves
/// none, and it is exactly the answer that turns a refusal into an offer. Left out, every
/// checkout with panes in it answers "still reading that working tree" for the life of the
/// picker, and nothing on screen or in the suite says why.
/// Carry out a removal the user has said yes to, and put what happened on the screen.
///
/// Out of the loop for the reason `show_answers` is: `run` needs a terminal and a keyboard,
/// so nothing in it is reachable from a test, and this is the arm with consequences. What
/// deleting it looks like is a picker where `y` closes the confirmation box and does
/// nothing else — no error, no message, the row unchanged — which is a shape this
/// repository has shipped once already.
fn start_removal(
    state: &mut PanesState,
    dirty: &mut Dirty,
    removals: &mut Removals,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    removal: &Removal,
) {
    match removals.remove(herdr, removal) {
        Ok(()) => {
            // The row says what is happening to it; nothing to add here.
            state.set_removing(removals.paths());
            // And only here: the panes are gone, so the list is known wrong rather than
            // merely possibly stale. An empty checkout's removal changes nothing on screen
            // yet and leaves the cursor where it was, which is where the next thing to tidy
            // up usually is.
            if !removal.panes().is_empty() {
                match collect::collect_tree(herdr, git) {
                    Ok((_, tree)) => {
                        state.replace_tree(tree);
                        dirty.ask(state.tree());
                    }
                    // Not fatal, but not silent either: rows for panes that have certainly
                    // stopped are on screen until this works.
                    Err(error) => state.set_message(format!(
                        "the panes closed, but the list could not be read again: {error:#}"
                    )),
                }
            }
        }
        // Nothing else is said or done. This is the account of what happened, and a failed
        // re-read would overwrite it with a sentence about panes that may not have closed
        // at all.
        Err(message) => state.set_message(message),
    }
}

fn show_answers(state: &mut PanesState, dirty: &mut Dirty) -> bool {
    if dirty.drain() {
        state.set_dirty(dirty.paths());
        state.set_unreadable(dirty.unreadable());
    }
    let reading = dirty.is_waiting();
    state.set_waiting(reading);
    state.set_answered(dirty.answered());
    reading
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
    use crate::app::fakes::{until, Recorder, Refuses, Started};
    use crate::ui::state::PanesState;
    use anyhow::Result;

    /// Answers every working tree at once and calls it clean, which is what makes the
    /// difference between "asked" and "answered" observable in one drain.
    /// A git that gives the same answer for every working tree, or the same failure.
    /// All three matter: clean is what lets a removal through, and dirty and unreadable are
    /// what the two refusals protecting a working agent are made of.
    struct Answers(Option<bool>);

    impl GitPort for Answers {
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            self.0
                .ok_or_else(|| anyhow::anyhow!("fatal: not a git repository"))
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!()
        }
        fn github_slug(&self, _repo_root: &str) -> Result<Option<String>> {
            unreachable!()
        }
        fn local_refs(&self, _repo_root: &str) -> Result<Vec<crate::port::GitRef>> {
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

    fn no_pane_tree() -> crate::domain::model::Tree {
        let mut tree = one_pane_tree();
        tree.repos[0].worktrees[0].panes.clear();
        tree
    }

    fn one_pane_tree() -> crate::domain::model::Tree {
        use crate::domain::model::{PaneNode, RepoNode, Tree, WorktreeNode};
        Tree {
            repos: vec![RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                worktrees: vec![WorktreeNode {
                    branch: Some("feat/login".into()),
                    checkout_path: "/wt/feat-login".into(),
                    is_primary: false,
                    open_workspace_id: Some("w2".into()),
                    track: None,
                    panes: vec![PaneNode {
                        pane_id: "w2:p1".into(),
                        workspace_id: "w2".into(),
                        tab_id: "w2:t1".into(),
                        display_name: Some("codex".into()),
                        agent_status: crate::port::AgentStatus::Idle,
                        focused: false,
                    }],
                }],
            }],
            ungrouped: Vec::new(),
        }
    }

    #[test]
    fn the_loop_hands_on_a_dirty_answer_too_or_nothing_is_ever_protected() {
        // The negative twin of the test below, and the one with teeth. `set_answered` alone
        // is what clears "still reading"; the dirty answer is what the refusal is made of.
        // Hand on the first without the second and `Shift-D` walks straight into the
        // confirmation box for a checkout full of working agents. `y` then closes every one
        // of their panes before git refuses to remove the checkout — so git saves the work
        // and nothing saves the agents.
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(true))));
        dirty.ask(state.tree());

        until(
            "the walk never answered for the only checkout there is",
            || !show_answers(&mut state, &mut dirty),
        );

        state.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('D'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(
            state.pending_removal().is_none(),
            "a checkout holding uncommitted work is not offered"
        );
        assert_eq!(
            state.message(),
            Some("that checkout is holding work nobody has committed"),
            "and it says which of the refusals this is"
        );
    }

    /// The checkout the one-pane tree describes, ready to be removed.
    fn only_removal(state: &PanesState) -> Removal {
        let repo = &state.tree().repos[0];
        Removal::of(&repo.repo_root, &repo.worktrees[0])
    }

    #[test]
    fn a_yes_starts_the_removal_and_says_on_the_row_that_it_is_going() {
        // The arm behind `y`. Deleted, the confirmation box closes and nothing happens —
        // no error, no message, the row unchanged — and the whole gate stays green.
        let recorder = Recorder::default();
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        let removal = only_removal(&state);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &recorder,
            &Answers(Some(false)),
            &removal,
        );

        assert_eq!(recorder.did(), ["start /wt/feat-login after 0"]);
        assert_eq!(removals.paths(), ["/wt/feat-login".to_string()]);
        assert!(
            state.rows().iter().any(|row| row.is_removing),
            "and the row it is happening to says so"
        );
        assert_eq!(state.message(), None, "the row is the report");
    }

    #[test]
    fn a_removal_that_will_not_start_puts_the_reason_where_the_user_is_looking() {
        // Ignoring the error is the narrower version of deleting the arm, and worse than it
        // looks: the panes are already closed by the time this can fail.
        let recorder = Recorder::default();
        let mut removals = Removals::new(&Refuses);
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        let removal = only_removal(&state);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &recorder,
            &Answers(Some(false)),
            &removal,
        );

        assert_eq!(
            state.message(),
            Some(
                "could not start removing feat/login: could not spawn: no such file or \
                 directory"
            )
        );
        assert!(!state.rows().iter().any(|row| row.is_removing));
    }

    #[test]
    fn a_working_tree_git_would_not_read_is_handed_on_as_its_own_refusal() {
        // Not the same as dirty and not the same as clean. Folded into either, a checkout
        // git could not answer for is offered for removal on the strength of an answer
        // nobody gave.
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(None)));
        dirty.ask(state.tree());

        until(
            "the walk never answered for the only checkout there is",
            || !show_answers(&mut state, &mut dirty),
        );

        state.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('D'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("git would not read that working tree")
        );
    }

    #[test]
    fn the_loop_hands_on_what_the_walk_answered_or_nothing_can_be_deleted() {
        // The wiring that shipped dead once, and would ship dead again silently: with the
        // answers never handed on, every checkout with panes says "still reading that
        // working tree" for the life of the picker and the feature is unreachable.
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        dirty.ask(state.tree());

        until(
            "the walk never answered for the only checkout there is",
            || !show_answers(&mut state, &mut dirty),
        );

        // The cursor starts on the only pane there is.
        state.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('D'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(
            state.pending_removal().is_some(),
            "the walk answered, so the question can be asked: {:?}",
            state.message()
        );
    }
}
