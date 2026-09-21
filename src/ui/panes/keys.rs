//! What every key does, and the order the modes are asked in.
//!
//! [`PanesState::handle_key`] is that order and nothing else: a question on screen, then the
//! panel over the list, then the keys that are the same in every mode, then the search box,
//! then the sweep, then the ordinary list. Each of those is its own function, so a key that
//! two modes disagree about is a difference between two functions rather than a branch
//! halfway down one.
//!
//! All of it is pure: a key and the current state go in, an [`Action`] comes out, and
//! anything that touches herdr is returned for the loop to do once the terminal is back.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::domain::rows::{RowRef, StateFilter};
use crate::ui::panes::*;

impl PanesState {
    /// What Enter means on the current row.
    ///
    /// Every row the cursor can reach stands for somewhere to go, so this is total in the
    /// only two ways that matter: go to a pane, or open a checkout that has none.
    fn activate(&mut self) -> Action {
        let Some(row) = self.selected() else {
            return Action::Consumed;
        };
        match row.reference {
            RowRef::Pane(r, w, p) => {
                Action::Jump(self.tree.repos[r].worktrees[w].panes[p].pane_id.clone())
            }
            RowRef::Ungrouped(p) => Action::Jump(self.tree.ungrouped[p].pane_id.clone()),
            RowRef::Worktree(r, w) => {
                let repo = &self.tree.repos[r];
                let worktree = &repo.worktrees[w];
                match worktree.panes.first() {
                    // A checkout that is already being worked in: go to the work rather
                    // than opening a second copy of it. Unreachable while the cursor stops
                    // only on idle checkouts, but the panes are right there either way.
                    Some(pane) => Action::Jump(pane.pane_id.clone()),
                    None => Action::OpenWorktree {
                        repo_root: repo.repo_root.clone(),
                        checkout_path: worktree.checkout_path.clone(),
                    },
                }
            }
            // Headings. The cursor does not stop on them.
            RowRef::Repo(_) | RowRef::UngroupedRepo => Action::Consumed,
        }
    }

    /// What `Shift-D` means: offer to delete the checkout under the cursor — or, on a pane,
    /// the checkout that pane is in, which is how a checkout with panes is reached at all,
    /// since the cursor does not stop on one.
    ///
    /// The refusals happen here rather than after the question, because asking "are you
    /// sure?" about something that cannot happen is worse than saying so — and because for
    /// a checkout with panes, "after the question" is after they have been closed.
    fn ask_to_remove(&mut self) -> Action {
        let Some((r, w)) = self.selected_worktree() else {
            self.message = Some("select a checkout, or a pane in one".into());
            return Action::Consumed;
        };
        let repo = &self.tree.repos[r];
        let worktree = &repo.worktrees[w];
        if worktree.is_primary {
            // git cannot remove the main working tree, and it is not a worktree anyway.
            self.message = Some("that is the repository itself, not a worktree".into());
            return Action::Consumed;
        }
        if self.options.removing.contains(&CheckoutPath::of(worktree)) {
            // A second one would race the first, and would close panes that the first is
            // already having removed out from under them.
            self.message = Some("that checkout is already being removed".into());
            return Action::Consumed;
        }
        // The refusals below are only reached by a checkout with panes in it, and that is
        // the whole reason they exist: for an empty one there is nothing to lose by letting
        // git answer for itself, but here the panes are gone by the time it speaks.
        if !worktree.panes.is_empty() {
            // `None` is not `Clean`: only an answer is a licence to close somebody's panes.
            // The walk lands after the first frame, so `None` is the ordinary state of the
            // checkout the picker opens on — the one the cursor is already sitting in.
            let refusal = match self.options.working_trees.get(&CheckoutPath::of(worktree)) {
                Some(WorkingTree::Clean) => None,
                Some(WorkingTree::Dirty) => {
                    Some("that checkout is holding work nobody has committed")
                }
                Some(WorkingTree::Unreadable) => Some("git would not read that working tree"),
                None => Some("still reading that working tree — try again"),
            };
            if let Some(refusal) = refusal {
                self.message = Some(refusal.into());
                return Action::Consumed;
            }
        }
        self.pending_removal = Some(Removal::of(&repo.repo_root, worktree));
        Action::Consumed
    }

    /// What `n` means on the current row: add a pane to this checkout.
    fn new_pane(&mut self) -> Action {
        let Some((r, w)) = self.selected_worktree() else {
            self.message = Some("select a worktree or a pane first".into());
            return Action::Consumed;
        };
        let worktree = &self.tree.repos[r].worktrees[w];
        match worktree.panes.first() {
            Some(pane) => Action::NewPane {
                checkout_path: worktree.checkout_path.clone(),
                beside_pane_id: pane.pane_id.clone(),
            },
            // With no pane there is nothing to split, so this is the same as opening it.
            None => Action::OpenWorktree {
                repo_root: self.tree.repos[r].repo_root.clone(),
                checkout_path: worktree.checkout_path.clone(),
            },
        }
    }

    /// Narrow to one agent state, or clear the filter. Mirrors the navigator's b/w/i/d/a.
    fn set_state_filter(&mut self, filter: Option<StateFilter>) -> Action {
        // Pressing the same key again clears, so a filter is never a one-way door.
        self.options.state_filter = if self.options.state_filter == filter {
            None
        } else {
            filter
        };
        let anchor = self.selected_pane_id().map(str::to_string);
        self.rebuild(anchor.as_deref());
        Action::Consumed
    }

    /// What `!` means: put every condition on screen in full.
    ///
    /// With nothing wrong it says so rather than opening an empty panel. The user pressed a
    /// key to ask a question, and "nothing" is an answer to it; a panel with no lines in it
    /// is not.
    fn show_conditions(&mut self) -> Action {
        if self.conditions().is_empty() {
            self.message = Some("nothing is wrong".into());
        } else {
            self.showing_conditions = true;
        }
        Action::Consumed
    }

    /// The one place that says which mode a key belongs to, and in what order to ask.
    ///
    /// Order is the whole of it. A question on screen comes first because it is the only
    /// thing the keyboard is for while it is up; the panel next, for the same reason; then
    /// the keys that mean the same in every mode; then the search box, which turns letters
    /// into text; then the sweep; then the list. Each mode is a function of its own, so a
    /// key two of them disagree about is a difference between two functions rather than a
    /// branch halfway down one.
    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Windows sends both press and release; only act on press.
        if key.kind == KeyEventKind::Release {
            return Action::Ignored;
        }
        self.message = None;

        if let Some(action) = self.answer_question(key) {
            return action;
        }
        if self.showing_conditions {
            return self.handle_panel_key(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.handle_control_key(key);
        }
        if self.filtering {
            return self.handle_filter_key(key);
        }
        if self.is_sweeping() {
            return self.handle_sweep_key(key);
        }
        self.handle_list_key(key)
    }

    /// A box is on screen and it is the only thing the keyboard is for.
    ///
    /// `None` when there is no box, which is what lets the caller go on to the modes under
    /// it. Taking the question out of the state before matching is what makes "anything
    /// else is a no" true of keys nobody thought of as well as of the ones they did.
    fn answer_question(&mut self, key: KeyEvent) -> Option<Action> {
        if let Some(removal) = self.pending_removal.take() {
            return Some(match key.code {
                KeyCode::Char('y') => Action::RemoveWorktree(removal),
                _ => Action::Consumed,
            });
        }

        if let Some(sweep) = self.pending_sweep.take() {
            return Some(match key.code {
                KeyCode::Char('y') => {
                    // The sweep ends with its question: what `y` removes is what the box
                    // listed, and marks left behind would be marks the box was not about.
                    self.set_sweeping(false);
                    Action::RemoveWorktrees(sweep)
                }
                _ => Action::Consumed,
            });
        }
        None
    }

    /// The panel holding every condition in full, while it is over the list.
    ///
    /// It holds the whole of what the prompt line could only summarise, so while it is up it
    /// is what the keyboard is for. It closes on the key that opened it, and on the keys
    /// that mean "go back" everywhere else in the picker.
    fn handle_panel_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            // Abandoning the picker outright is not a key the panel is allowed to swallow:
            // it is how a pane is got rid of when nothing else is working.
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::Quit
            }
            KeyCode::Char('!') | KeyCode::Esc | KeyCode::Char('q') => {
                self.showing_conditions = false
            }
            // Anything else is left alone rather than acted on: a key pressed at a panel was
            // aimed at the panel, and the list under it is not what the user is looking at.
            _ => {}
        }
        Action::Consumed
    }

    /// The keys that mean the same thing in every mode, because the terminal's own habits
    /// do not change with what the picker is showing.
    fn handle_control_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('c') => Action::Quit,
            KeyCode::Char('n') => {
                self.move_cursor(1);
                Action::Consumed
            }
            KeyCode::Char('p') => {
                self.move_cursor(-1);
                Action::Consumed
            }
            // The navigator clears its search with ctrl+u.
            KeyCode::Char('u') if self.filtering => {
                self.options.query.clear();
                self.rebuild(None);
                Action::Consumed
            }
            _ => Action::Ignored,
        }
    }

    /// A sweep is a mode, and the keys that mean something in it are its own. Only the ones
    /// that move the cursor are shared, because a list you cannot walk is a list you cannot
    /// decide about.
    fn handle_sweep_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            // Leaving is not quitting. `q` out of a sweep puts the picker back rather than
            // closing it, because the sweep is what the user opened last and it is what
            // they are getting out of.
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('S') => self.set_sweeping(false),
            KeyCode::Char(' ') => self.flip_mark(),
            // Reading is not deciding, so this one key is shared with the ordinary mode:
            // `gh` refusing is a sweep's own condition, and a sweep is when the user most
            // needs to read it whole.
            KeyCode::Char('!') => self.show_conditions(),
            KeyCode::Enter => {
                let chosen = self.chosen();
                if chosen.is_empty() {
                    self.message = Some("nothing is marked".into());
                    return Action::Consumed;
                }
                // Not asked yet: the marks rest on facts from when the picker opened or was
                // last reloaded, and the loop reads them again first —
                // `confirm_sweep_if_settled` asks once the walk has answered. Issue #25.
                if let Some(sweeping) = self.sweep.as_mut() {
                    sweeping.confirming = Some(chosen.into_iter().collect());
                }
                Action::SweepReload
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                Action::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                Action::Consumed
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.move_group(1);
                Action::Consumed
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.move_group(-1);
                Action::Consumed
            }
            // Everything else, deliberately. A sweep is the one mode where a key that half
            // works is dangerous: `D` here would ask about one checkout while the marks say
            // something about twenty, and `Tab` would leave marks behind on a screen the
            // user has left.
            _ => Action::Ignored,
        }
    }

    /// The ordinary mode: the list, and everything that acts on the row under the cursor.
    fn handle_list_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                Action::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                Action::Consumed
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.move_group(1);
                Action::Consumed
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.move_group(-1);
                Action::Consumed
            }
            KeyCode::Enter => self.activate(),
            KeyCode::Char('n') => self.new_pane(),
            KeyCode::Char('p') => {
                self.set_show_worktrees_without_panes(!self.shows_worktrees_without_panes());
                Action::Consumed
            }
            KeyCode::Char('r') => Action::Reload,
            KeyCode::Char('!') => self.show_conditions(),
            KeyCode::Char('b') => self.set_state_filter(Some(StateFilter::Blocked)),
            KeyCode::Char('w') => self.set_state_filter(Some(StateFilter::Working)),
            KeyCode::Char('i') => self.set_state_filter(Some(StateFilter::Idle)),
            KeyCode::Char('d') => self.set_state_filter(Some(StateFilter::Done)),
            KeyCode::Char('a') => self.set_state_filter(None),
            KeyCode::Char('/') => {
                self.filtering = true;
                Action::Consumed
            }
            KeyCode::Char('D') => self.ask_to_remove(),
            // `Shift-S` beside `Shift-D`: the two keys that delete things are both shifted.
            KeyCode::Char('S') => self.set_sweeping(true),
            // A preselection, not a requirement: the branches picker asks which repository
            // anyway.
            KeyCode::Tab => Action::ShowBranches {
                repo_root: self
                    .selected_repo_index()
                    .map(|index| self.tree.repos[index].repo_root.clone()),
            },
            _ => Action::Ignored,
        }
    }

    /// The search box: letters are text rather than commands, and the arrows still walk the
    /// list under it.
    fn handle_filter_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            // Esc abandons the search; Enter keeps the filter and returns to commands.
            KeyCode::Esc => {
                self.filtering = false;
                self.options.query.clear();
                self.rebuild(None);
                Action::Consumed
            }
            KeyCode::Enter => {
                self.filtering = false;
                Action::Consumed
            }
            KeyCode::Backspace => {
                self.options.query.pop();
                self.rebuild(None);
                Action::Consumed
            }
            KeyCode::Down => {
                self.move_cursor(1);
                Action::Consumed
            }
            KeyCode::Up => {
                self.move_cursor(-1);
                Action::Consumed
            }
            // Arrows are not text, so they keep working while the search box has focus.
            KeyCode::Right => {
                self.move_group(1);
                Action::Consumed
            }
            KeyCode::Left => {
                self.move_group(-1);
                Action::Consumed
            }
            KeyCode::Char(c) => {
                self.options.query.push(c);
                self.rebuild(None);
                Action::Consumed
            }
            _ => Action::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{Refs, RepoNode, Tree, WorkingTree, WorktreeNode};
    use crate::port::AgentStatus;
    use crate::ui::panes::fixtures::*;

    #[test]
    fn enter_on_a_pane_jumps_to_it() {
        let mut state = state();
        select(&mut state, "codex");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Action::Jump("w2:p1".into())
        );
    }

    #[test]
    fn enter_on_a_worktree_that_is_already_open_goes_to_its_work() {
        // The cursor does not stop here; the panes listed under it are what you pick.
        let mut state = state();
        select(&mut state, "main");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Action::Jump("w1:p1".into())
        );
    }

    #[test]
    fn enter_on_a_worktree_with_no_pane_opens_the_checkout() {
        let mut state = state();
        select(&mut state, "fix/crash");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Action::OpenWorktree {
                repo_root: "/src/app".into(),
                checkout_path: "/wt/app/fix-crash".into(),
            }
        );
    }

    #[test]
    fn the_cursor_visits_only_panes_and_checkouts_with_nothing_running() {
        let mut state = state();
        let first = state.selected().unwrap().name.clone();
        let mut stops = vec![first.clone()];
        for _ in 0..state.lines().len() {
            state.handle_key(key(KeyCode::Down));
            let label = state.selected().unwrap().name.clone();
            if label == first {
                break;
            }
            stops.push(label);
        }
        assert_eq!(
            stops,
            [
                Some("claude".to_string()),
                Some("codex".to_string()),
                Some("fix/crash".to_string()),
                Some("zsh".to_string())
            ]
        );

        // And the rows it stepped over are still on screen.
        assert_eq!(
            row_labels(&state),
            [
                "me/app (2)",
                "main",
                "claude",
                "feat/login",
                "codex",
                "fix/crash",
                "not in any repository (1)",
                "zsh",
            ]
        );
    }

    #[test]
    fn the_arrows_move_between_repositories() {
        // The fixture has one repository plus the panes in none of them.
        let mut state = state();
        assert_eq!(state.selected().unwrap().name.as_deref(), Some("claude"));

        state.handle_key(key(KeyCode::Right));
        assert_eq!(
            state.selected().unwrap().name.as_deref(),
            Some("zsh"),
            "the panes in no repository are a section like any other"
        );
        state.handle_key(key(KeyCode::Right));
        assert_eq!(
            state.selected().unwrap().name.as_deref(),
            Some("claude"),
            "and it wraps"
        );

        // From deep inside a group, one press still leaves it.
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected().unwrap().name.as_deref(), Some("fix/crash"));
        state.handle_key(key(KeyCode::Left));
        assert_eq!(state.selected().unwrap().name.as_deref(), Some("zsh"));
    }

    #[test]
    fn h_and_l_are_the_arrows_by_another_name() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('l')));
        assert_eq!(state.selected().unwrap().name.as_deref(), Some("zsh"));
        state.handle_key(key(KeyCode::Char('h')));
        assert_eq!(state.selected().unwrap().name.as_deref(), Some("claude"));
    }

    #[test]
    fn the_arrows_keep_working_while_the_search_box_has_focus() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        assert!(state.is_filtering());
        state.handle_key(key(KeyCode::Right));
        assert_eq!(state.selected().unwrap().name.as_deref(), Some("zsh"));
        assert_eq!(state.query(), "", "an arrow is not text");

        // A letter is, though: `l` types rather than moves once the search box has focus.
        state.handle_key(key(KeyCode::Char('l')));
        assert_eq!(state.query(), "l");
    }

    #[test]
    fn moving_up_from_the_top_wraps_to_the_last_thing_worth_going_to() {
        let mut state = state();
        state.handle_key(key(KeyCode::Up));
        assert_eq!(state.selected().unwrap().name.as_deref(), Some("zsh"));
    }

    #[test]
    fn n_adds_a_pane_beside_the_work_already_in_that_checkout() {
        let mut state = state();
        select(&mut state, "codex");
        assert_eq!(
            state.handle_key(key(KeyCode::Char('n'))),
            Action::NewPane {
                checkout_path: "/wt/app/feat-login".into(),
                beside_pane_id: "w2:p1".into(),
            }
        );
    }

    #[test]
    fn n_on_a_checkout_with_no_pane_opens_it_since_there_is_nothing_to_split() {
        let mut state = state();
        select(&mut state, "fix/crash");
        assert!(matches!(
            state.handle_key(key(KeyCode::Char('n'))),
            Action::OpenWorktree { .. }
        ));
    }

    #[test]
    fn shift_d_asks_before_it_deletes_anything() {
        let mut state = state();
        select(&mut state, "fix/crash");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);

        let asked = state.pending_removal().expect("a question should be up");
        assert_eq!(asked.label(), "fix/crash");
        assert_eq!(asked.checkout_path().as_str(), "/wt/app/fix-crash");
        assert_eq!(asked.repo_root(), "/src/app");

        let Action::RemoveWorktree(asked) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        assert_eq!(asked.repo_root(), "/src/app");
        assert_eq!(asked.checkout_path().as_str(), "/wt/app/fix-crash");
        assert_eq!(asked.label(), "fix/crash");
        assert!(asked.panes().is_empty(), "there were none to close");
        assert!(!asked.delete_branch(), "Shift-D keeps the branch");
        assert!(
            state.pending_removal().is_none(),
            "the question is answered"
        );
    }

    #[test]
    fn anything_that_is_not_y_is_a_no() {
        // Including keys nobody thought of.
        for code in [
            KeyCode::Char('n'),
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Char('D'),
            KeyCode::Down,
            KeyCode::Char('Y'),
        ] {
            let mut state = state();
            select(&mut state, "fix/crash");
            state.handle_key(key(KeyCode::Char('D')));
            assert_eq!(state.handle_key(key(code)), Action::Consumed, "{code:?}");
            assert!(state.pending_removal().is_none(), "{code:?}");
        }
    }

    #[test]
    fn shift_d_on_a_pane_offers_to_delete_the_checkout_it_is_in() {
        let mut state = state();
        // Two panes, so the assertion below can tell "the checkout's panes" from "the first
        // pane in the checkout", and can see them reordered.
        let mut tree = state.tree().clone();
        tree.repos[0].worktrees[1]
            .panes
            .push(pane("w2:p2", "zsh", AgentStatus::Unknown));
        state.replace_tree(tree);
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Clean)]));
        select(&mut state, "codex");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);

        let asked = state.pending_removal().expect("a question should be up");
        assert_eq!(asked.label(), "feat/login");
        let closing: Vec<&str> = asked.panes().iter().map(|p| p.pane_id.as_str()).collect();
        assert_eq!(closing, ["w2:p1", "w2:p2"], "and it names what stops");

        // And `y` carries them through in the order the question listed them: otherwise the
        // picker asks about panes it never closes.
        let Action::RemoveWorktree(asked) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        assert_eq!(asked.checkout_path().as_str(), "/wt/app/feat-login");
        let closing: Vec<&str> = asked.panes().iter().map(|p| p.pane_id.as_str()).collect();
        assert_eq!(closing, ["w2:p1", "w2:p2"]);
    }

    #[test]
    fn a_checkout_with_panes_whose_working_tree_has_not_answered_yet_is_refused() {
        // The state the picker opens in: nothing has answered yet, and the cursor is already
        // on the pane the user came from. `r` puts every checkout back into it, so this is
        // not only a startup window.
        let mut state = state();
        select(&mut state, "codex");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("still reading that working tree — try again")
        );

        // An empty checkout is offered either way: nothing is at stake before the question,
        // and git answers for itself.
        select(&mut state, "fix/crash");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(state.pending_removal().is_some());
    }

    #[test]
    fn a_checkout_already_being_removed_is_not_offered_again() {
        // The checkout's row stops being selectable but its panes' rows do not, so a second
        // confirmation is still reachable from a pane.
        let mut state = state();
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Clean)]));
        state.set_removing(vec![CheckoutPath::for_test("/wt/app/feat-login")]);
        select(&mut state, "codex");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("that checkout is already being removed")
        );
    }

    #[test]
    fn a_checkout_with_panes_is_refused_before_the_question_when_it_is_holding_work() {
        let mut state = state();
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Dirty)]));
        select(&mut state, "codex");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("that checkout is holding work nobody has committed")
        );
    }

    #[test]
    fn a_checkout_with_panes_is_refused_when_git_would_not_read_it() {
        // Which is not the same as reading it and finding nothing: without an answer there
        // is no protection to offer, and the panes would go on the strength of a guess.
        let mut state = state();
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Unreadable)]));
        select(&mut state, "codex");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("git would not read that working tree")
        );
    }

    #[test]
    fn an_empty_checkout_holding_work_is_still_offered_and_left_to_git() {
        // Nothing is at stake before the question here, and git's refusal is the answer
        // rather than an obstacle — it says what would have been lost.
        let mut state = state();
        state.set_working_trees(answers(&[("/wt/app/fix-crash", WorkingTree::Dirty)]));
        select(&mut state, "fix/crash");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(state.pending_removal().is_some());
    }

    #[test]
    fn the_repositorys_own_checkout_is_never_offered() {
        let mut on_a_pane = state();
        select(&mut on_a_pane, "claude");
        assert_eq!(
            on_a_pane.handle_key(key(KeyCode::Char('D'))),
            Action::Consumed
        );
        assert!(on_a_pane.pending_removal().is_none());
        assert!(on_a_pane.message().is_some(), "and it says why");

        // Nothing running in it, so the cursor can reach the checkout's own row.
        let mut on_the_repo = PanesState::new(
            Tree {
                repos: vec![RepoNode {
                    repo_key: "/src/app/.git".into(),
                    repo_root: "/src/app".into(),
                    display_name: "me/app".into(),
                    refs: Refs::Read,
                    worktrees: vec![
                        WorktreeNode {
                            branch: Some("main".into()),
                            checkout_path: "/src/app".into(),
                            is_primary: true,
                            open_workspace_id: None,
                            track: None,
                            panes: vec![],
                        },
                        WorktreeNode {
                            branch: Some("feat/login".into()),
                            checkout_path: "/wt/app/feat-login".into(),
                            is_primary: false,
                            open_workspace_id: Some("w2".into()),
                            track: None,
                            panes: vec![pane("w2:p1", "codex", AgentStatus::Blocked)],
                        },
                    ],
                }],
                ungrouped: vec![],
            },
            None,
        );
        select(&mut on_the_repo, "main");
        on_the_repo.handle_key(key(KeyCode::Char('D')));
        assert!(on_the_repo.pending_removal().is_none());
        assert!(
            on_the_repo
                .message()
                .unwrap_or_default()
                .contains("the repository itself"),
            "got {:?}",
            on_the_repo.message()
        );
    }

    #[test]
    fn tab_asks_for_the_branches_of_the_repository_the_cursor_is_in() {
        let mut state = state();
        select(&mut state, "codex");
        assert_eq!(
            state.handle_key(key(KeyCode::Tab)),
            Action::ShowBranches {
                repo_root: Some("/src/app".into())
            }
        );
    }

    #[test]
    fn the_state_keys_narrow_to_one_agent_state_and_a_clears_them() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('b')));
        assert_eq!(state.state_filter(), Some(StateFilter::Blocked));
        assert_eq!(row_labels(&state), ["me/app (2)", "feat/login", "codex"]);

        state.handle_key(key(KeyCode::Char('w')));
        assert_eq!(state.state_filter(), Some(StateFilter::Working));
        assert_eq!(row_labels(&state), ["me/app (2)", "main", "claude"]);

        state.handle_key(key(KeyCode::Char('a')));
        assert_eq!(state.state_filter(), None);
        assert!(row_labels(&state).contains(&"fix/crash".to_string()));
    }

    #[test]
    fn pressing_the_same_state_key_twice_clears_it_so_it_is_never_a_one_way_door() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('b')));
        assert_eq!(state.state_filter(), Some(StateFilter::Blocked));
        state.handle_key(key(KeyCode::Char('b')));
        assert_eq!(state.state_filter(), None);
    }

    #[test]
    fn p_toggles_worktrees_without_panes() {
        let mut state = state();
        state.set_show_worktrees_without_panes(false);
        assert!(!row_labels(&state).contains(&"fix/crash".to_string()));

        assert_eq!(state.handle_key(key(KeyCode::Char('p'))), Action::Consumed);
        assert!(state.shows_worktrees_without_panes());
        assert!(row_labels(&state).contains(&"fix/crash".to_string()));

        state.handle_key(key(KeyCode::Char('p')));
        assert!(!state.shows_worktrees_without_panes());
        assert!(!row_labels(&state).contains(&"fix/crash".to_string()));
    }

    #[test]
    fn p_keeps_the_cursor_on_a_still_visible_pane() {
        let mut state = state();
        // This pane follows the row that disappears, so this also checks the cursor repair.
        select(&mut state, "zsh");

        assert_eq!(state.handle_key(key(KeyCode::Char('p'))), Action::Consumed);
        assert_eq!(cursor_label(&state), "zsh");
    }

    #[test]
    fn hide_moves_the_cursor_off_a_worktree_that_disappears() {
        let mut state = state();
        select(&mut state, "fix/crash");
        state.set_show_worktrees_without_panes(false);
        assert_ne!(cursor_label(&state), "fix/crash");
        assert!(rows::selectable(
            state.rows(),
            state.lines(),
            state.cursor()
        ));
    }

    #[test]
    fn slash_starts_searching_and_typing_narrows_the_list() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        assert!(state.is_filtering());
        for c in "codex".chars() {
            state.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(state.query(), "codex");
        assert_eq!(row_labels(&state), ["me/app (2)", "feat/login", "codex"]);
    }

    #[test]
    fn while_searching_letters_are_text_rather_than_commands() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        // `b` would apply a state filter outside search mode.
        assert_eq!(state.handle_key(key(KeyCode::Char('b'))), Action::Consumed);
        assert_eq!(state.query(), "b");
        assert_eq!(state.state_filter(), None);
    }

    #[test]
    fn ctrl_u_clears_the_search_the_way_the_navigator_does() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        for c in "code".chars() {
            state.handle_key(key(KeyCode::Char(c)));
        }
        state.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(state.query(), "");
        assert!(state.is_filtering(), "still searching, just empty");
    }

    #[test]
    fn escape_abandons_the_search_but_enter_keeps_it() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        state.handle_key(key(KeyCode::Char('c')));
        state.handle_key(key(KeyCode::Enter));
        assert!(!state.is_filtering());
        assert_eq!(state.query(), "c");

        state.handle_key(key(KeyCode::Char('/')));
        state.handle_key(key(KeyCode::Esc));
        assert!(!state.is_filtering());
        assert_eq!(state.query(), "");
    }

    #[test]
    fn escape_and_q_quit_when_not_searching() {
        assert_eq!(state().handle_key(key(KeyCode::Esc)), Action::Quit);
        assert_eq!(state().handle_key(key(KeyCode::Char('q'))), Action::Quit);
        assert_eq!(
            state().handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }

    #[test]
    fn moving_the_cursor_never_lands_on_a_blank_line() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('h')));
        for _ in 0..state.lines().len() * 2 {
            state.handle_key(key(KeyCode::Char('j')));
            assert!(
                matches!(state.lines()[state.cursor()], DisplayLine::Row(_)),
                "cursor landed on a blank line"
            );
        }
        for _ in 0..state.lines().len() * 2 {
            state.handle_key(key(KeyCode::Char('k')));
            assert!(matches!(state.lines()[state.cursor()], DisplayLine::Row(_)));
        }
    }

    #[test]
    fn the_key_that_reads_what_is_wrong_answers_when_nothing_is() {
        // A panel with no lines in it is not an answer to a question the user asked with a
        // keypress, and a key that appears to do nothing is the same bug as a blank line.
        let mut state = state();
        assert!(state.conditions().is_empty());

        assert_eq!(state.handle_key(key(KeyCode::Char('!'))), Action::Consumed);
        assert!(!state.is_showing_conditions());
        assert_eq!(state.message(), Some("nothing is wrong"));
    }

    #[test]
    fn the_panel_opens_on_what_is_wrong_and_closes_three_ways() {
        for closing in [KeyCode::Char('!'), KeyCode::Esc, KeyCode::Char('q')] {
            let mut state = state();
            state.set_stale("herdr did not answer".into());

            assert_eq!(state.handle_key(key(KeyCode::Char('!'))), Action::Consumed);
            assert!(state.is_showing_conditions());
            // And `q` closes the panel rather than the picker, the way it leaves a sweep.
            assert_eq!(state.handle_key(key(closing)), Action::Consumed);
            assert!(!state.is_showing_conditions(), "{closing:?} closes it");
        }
    }

    #[test]
    fn a_key_the_panel_has_no_use_for_does_not_reach_the_list_under_it() {
        let mut state = state();
        state.set_stale("herdr did not answer".into());
        state.handle_key(key(KeyCode::Char('!')));
        let before = state.cursor;

        assert_eq!(state.handle_key(key(KeyCode::Char('j'))), Action::Consumed);
        assert_eq!(state.cursor, before, "the cursor is not where the user is");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);
        assert!(state.pending_removal().is_none(), "and nothing is asked");
        assert!(state.is_showing_conditions(), "the panel is still up");

        // Abandoning the picker is the one key it does not swallow.
        let quit = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(state.handle_key(quit), Action::Quit);
    }

    #[test]
    fn a_key_release_is_ignored_so_windows_does_not_act_twice() {
        let mut state = state();
        let release =
            KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Release);
        assert_eq!(state.handle_key(release), Action::Ignored);
    }
}
