//! The keymap. It is a step machine: which keys apply at all depends on which step the
//! picker is on, and on whether the search box has the keyboard.

use crate::domain::progress::Stage;
use crate::domain::resolve::{BranchState, Chosen};
use crate::ui::branches::list::is_branch_name;
use crate::ui::branches::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

impl BranchesState {
    pub fn handle_key(&mut self, key: KeyEvent) -> BranchAction {
        if key.kind == KeyEventKind::Release {
            return BranchAction::Ignored;
        }
        match &self.activity {
            Activity::Choosing => {}
            Activity::Working { stage, .. } => return self.handle_working_key(key, stage.clone()),
            Activity::Failed { .. } => return self.handle_failed_key(key),
        }
        self.message = None;

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') => BranchAction::Quit,
                KeyCode::Char('n') => {
                    self.move_cursor(1);
                    BranchAction::Consumed
                }
                KeyCode::Char('p') => {
                    self.move_cursor(-1);
                    BranchAction::Consumed
                }
                KeyCode::Char('u') => {
                    match self.step {
                        Step::Repo => {
                            self.repo_query.clear();
                            self.refilter_repos();
                        }
                        Step::Branch => {
                            self.query.clear();
                            self.refilter();
                        }
                        Step::Name => {
                            if let Some(naming) = &mut self.naming {
                                naming.name.clear();
                            }
                        }
                        Step::Destination => return BranchAction::Ignored,
                    }
                    BranchAction::Consumed
                }
                // The chords stay `o` and `r` rather than following the letters: in a
                // terminal `Ctrl-I` is Tab, which this view already spends on the panes.
                KeyCode::Char('o') if self.step == Step::Branch => {
                    self.reorder(self.order.cycle());
                    BranchAction::Consumed
                }
                KeyCode::Char('r') if self.step == Step::Branch => {
                    self.reorder(self.order.reverse());
                    BranchAction::Consumed
                }
                KeyCode::Char('f') if self.step == Step::Branch => BranchAction::Fetch {
                    repo_root: self.repo().repo_root.clone(),
                },
                _ => BranchAction::Ignored,
            };
        }

        match (self.step, self.filtering) {
            (Step::Repo, false) => self.handle_repo_key(key),
            (Step::Repo, true) => self.handle_repo_search_key(key),
            (Step::Branch, false) => self.handle_branch_key(key),
            (Step::Branch, true) => self.handle_branch_search_key(key),
            (Step::Name, _) => self.handle_name_key(key),
            (Step::Destination, _) => self.handle_destination_key(key),
        }
    }

    /// While a step is in flight the list is frozen. The only key that means anything is
    /// `Ctrl-C`, and only while stopping is free — see [`Stage::interruptible`].
    fn handle_working_key(&mut self, key: KeyEvent, stage: Stage) -> BranchAction {
        let interrupt =
            key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c'));
        if interrupt && stage.interruptible() {
            return BranchAction::Quit;
        }
        BranchAction::Ignored
    }

    /// The failure is on screen; any way of saying "I have read it" closes the picker.
    fn handle_failed_key(&mut self, key: KeyEvent) -> BranchAction {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('c') => {
                BranchAction::Quit
            }
            _ => BranchAction::Ignored,
        }
    }

    fn handle_repo_key(&mut self, key: KeyEvent) -> BranchAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => BranchAction::Quit,
            KeyCode::Tab => BranchAction::ShowPanes,
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                BranchAction::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                BranchAction::Consumed
            }
            KeyCode::Char('/') => {
                self.filtering = true;
                BranchAction::Consumed
            }
            KeyCode::Enter => self.enter_repo(),
            _ => BranchAction::Ignored,
        }
    }

    fn handle_repo_search_key(&mut self, key: KeyEvent) -> BranchAction {
        match key.code {
            // Esc abandons the search rather than keeping it, as it does in the panes view.
            KeyCode::Esc => {
                self.filtering = false;
                self.repo_query.clear();
                self.refilter_repos();
                BranchAction::Consumed
            }
            KeyCode::Down => {
                self.move_cursor(1);
                BranchAction::Consumed
            }
            KeyCode::Up => {
                self.move_cursor(-1);
                BranchAction::Consumed
            }
            KeyCode::Backspace => {
                self.repo_query.pop();
                self.refilter_repos();
                BranchAction::Consumed
            }
            // Enter picks rather than committing the search: what you do with a narrowed
            // list here is open the one thing left in it.
            KeyCode::Enter => self.enter_repo(),
            KeyCode::Char(c) => {
                self.repo_query.push(c);
                self.refilter_repos();
                BranchAction::Consumed
            }
            _ => BranchAction::Ignored,
        }
    }

    fn enter_repo(&mut self) -> BranchAction {
        let Some(index) = self.repo_visible.get(self.repo_cursor).copied() else {
            self.message = Some("no repository selected".into());
            return BranchAction::Consumed;
        };
        self.open_repo(index)
    }

    /// Move to a repository's branches. Whatever was on screen belonged to the repository
    /// being left, so it goes with it; the caller fills the gap before the next frame.
    fn open_repo(&mut self, index: usize) -> BranchAction {
        self.current = index;
        self.filtering = false;
        self.data = BranchData {
            loading: true,
            ..BranchData::default()
        };
        self.query.clear();
        self.step = Step::Branch;
        self.reresolve();
        self.cursor = 0;
        BranchAction::LoadRepo {
            repo_root: self.repos[index].repo_root.clone(),
        }
    }

    fn handle_branch_key(&mut self, key: KeyEvent) -> BranchAction {
        match key.code {
            KeyCode::Esc => self.leave_branches(),
            KeyCode::Char('q') => BranchAction::Quit,
            KeyCode::Tab => BranchAction::ShowPanes,
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                BranchAction::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                BranchAction::Consumed
            }
            KeyCode::Char('/') => {
                self.filtering = true;
                BranchAction::Consumed
            }
            KeyCode::Char('i') => {
                self.reorder(self.order.cycle());
                BranchAction::Consumed
            }
            // Shift arrives as the capital itself, whether or not the terminal also sets
            // the modifier, so the letter is what this matches on.
            KeyCode::Char('I') => {
                self.reorder(self.order.reverse());
                BranchAction::Consumed
            }
            KeyCode::Char('f') => BranchAction::Fetch {
                repo_root: self.repo().repo_root.clone(),
            },
            KeyCode::Char('n') => self.start_naming(),
            KeyCode::Enter => self.choose_branch(),
            _ => BranchAction::Ignored,
        }
    }

    fn handle_branch_search_key(&mut self, key: KeyEvent) -> BranchAction {
        match key.code {
            // Esc abandons the search rather than keeping it, as it does in the panes view.
            KeyCode::Esc => {
                self.filtering = false;
                self.query.clear();
                self.refilter();
                BranchAction::Consumed
            }
            KeyCode::Down => {
                self.move_cursor(1);
                BranchAction::Consumed
            }
            KeyCode::Up => {
                self.move_cursor(-1);
                BranchAction::Consumed
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
                BranchAction::Consumed
            }
            // Enter picks rather than committing the search, as on the repository step.
            KeyCode::Enter => self.choose_branch(),
            KeyCode::Char(c) => {
                self.query.push(c);
                self.refilter();
                BranchAction::Consumed
            }
            _ => BranchAction::Ignored,
        }
    }

    /// Esc from the branch list: back to the repositories, or out if there are none to
    /// choose between.
    fn leave_branches(&mut self) -> BranchAction {
        if self.has_repo_step {
            self.step = Step::Repo;
            self.filtering = false;
            BranchAction::Consumed
        } else {
            BranchAction::Quit
        }
    }

    fn choose_branch(&mut self) -> BranchAction {
        let Some(entry) = self.selected().cloned() else {
            self.message = Some("no branch selected".into());
            return BranchAction::Consumed;
        };
        // Already being worked on: go there. Asking where to put a second copy of work that
        // is already open would be the wrong question.
        if let BranchState::LivePane { pane_id, .. } = &entry.state {
            return BranchAction::Jump {
                pane_id: pane_id.clone(),
            };
        }
        self.chosen = Some(Chosen::Listed(entry));
        self.step = Step::Destination;
        self.filtering = false;
        self.destination_cursor = 0;
        BranchAction::Consumed
    }

    /// Start a branch from the row under the cursor. What it is cut from is decided here,
    /// before the name is asked for, so that the list can stay on screen without the cursor
    /// being able to change the answer.
    fn start_naming(&mut self) -> BranchAction {
        let Some(base) = self.selected().cloned() else {
            self.message = Some("no branch selected".into());
            return BranchAction::Consumed;
        };
        if base.state == BranchState::New {
            // Not reachable today: the offer to create is only on screen while something is
            // being typed, and letters are text there. It is guarded anyway because the
            // alternative is a `git worktree add` based on a ref that does not exist, and
            // because a search that survived its own keystrokes would make it reachable.
            self.message = Some(format!("{} does not exist yet", base.name));
            return BranchAction::Consumed;
        }
        self.filtering = false;
        self.naming = Some(Naming {
            base,
            name: String::new(),
        });
        self.step = Step::Name;
        BranchAction::Consumed
    }

    fn handle_name_key(&mut self, key: KeyEvent) -> BranchAction {
        match key.code {
            KeyCode::Esc => {
                self.step = Step::Branch;
                self.naming = None;
                BranchAction::Consumed
            }
            KeyCode::Enter => self.finish_naming(),
            KeyCode::Backspace => {
                if let Some(naming) = &mut self.naming {
                    naming.name.pop();
                }
                BranchAction::Consumed
            }
            KeyCode::Char(c) => {
                if let Some(naming) = &mut self.naming {
                    naming.name.push(c);
                }
                BranchAction::Consumed
            }
            _ => BranchAction::Ignored,
        }
    }

    /// Everything that would make git refuse is refused here instead, where it can be said
    /// in the prompt rather than surfacing as a failed step after the picker has committed.
    fn finish_naming(&mut self) -> BranchAction {
        let Some(naming) = self.naming.clone() else {
            return BranchAction::Ignored;
        };
        let name = naming.name.trim().to_string();
        if name.is_empty() {
            self.message = Some("name the branch first".into());
            return BranchAction::Consumed;
        }
        if !is_branch_name(&name) {
            self.message = Some(format!("git would refuse the name {name}"));
            return BranchAction::Consumed;
        }
        if self.entries.iter().any(|entry| entry.name == name) {
            self.message = Some(format!("{name} already exists here"));
            return BranchAction::Consumed;
        }
        self.chosen = Some(Chosen::NewFrom {
            name,
            base: naming.base,
        });
        self.step = Step::Destination;
        self.destination_cursor = 0;
        BranchAction::Consumed
    }

    fn handle_destination_key(&mut self, key: KeyEvent) -> BranchAction {
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => {
                self.step = if self.naming.is_some() {
                    Step::Name
                } else {
                    Step::Branch
                };
                self.chosen = None;
                BranchAction::Consumed
            }
            KeyCode::Char('q') => BranchAction::Quit,
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                BranchAction::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                BranchAction::Consumed
            }
            KeyCode::Enter => {
                // Acting would look like it worked and quietly do nothing, so say why not.
                if let Some(reason) = self.destination_is_blocked() {
                    self.message = Some(reason);
                    return BranchAction::Consumed;
                }
                let (Some(chosen), Some(destination)) = (
                    self.chosen.clone(),
                    self.destinations.get(self.destination_cursor).cloned(),
                ) else {
                    self.message = Some("no destination available".into());
                    return BranchAction::Consumed;
                };
                BranchAction::Chosen(Box::new(Choice {
                    chosen,
                    destination,
                }))
            }
            _ => BranchAction::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::branches::fixtures::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::domain::order::SortKey;

    #[test]
    fn typing_filters_once_the_search_box_has_been_opened() {
        let mut state = state();
        search(&mut state, "chore");
        assert_eq!(state.query(), "chore");
        assert_eq!(names(&state)[0], "chore/deps");
    }

    #[test]
    fn letters_are_commands_until_slash_is_pressed() {
        let mut state = state();
        assert!(!state.is_filtering());

        assert_eq!(
            state.handle_key(key(KeyCode::Char('j'))),
            BranchAction::Consumed
        );
        assert_eq!(state.cursor(), 1);
        state.handle_key(key(KeyCode::Char('k')));
        assert_eq!(state.cursor(), 0);
        assert_eq!(state.query(), "");

        // The order keys, which are letters rather than chords out here.
        state.handle_key(key(KeyCode::Char('i')));
        assert_eq!(state.order().key, SortKey::Updated);
        state.handle_key(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT));
        assert!(state.order().reversed);
        assert_eq!(
            state.handle_key(key(KeyCode::Char('f'))),
            BranchAction::Fetch {
                repo_root: "/src/app".into()
            }
        );
        assert_eq!(state.query(), "", "none of that was typing");

        // And `q` closes, as it does in the panes view.
        assert_eq!(
            state.handle_key(key(KeyCode::Char('q'))),
            BranchAction::Quit
        );
    }

    #[test]
    fn escape_abandons_the_search_and_gives_the_keyboard_back_to_the_list() {
        let mut state = state();
        search(&mut state, "chore");
        // The match, plus the offer to create a branch called `chore`.
        assert_eq!(names(&state), ["chore/deps", "chore"]);

        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Consumed);
        assert!(!state.is_filtering());
        assert_eq!(state.query(), "", "the filter goes with it");
        assert_eq!(names(&state).len(), 3);

        // A second Esc is the one that leaves, now that the search box has let go.
        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Quit);
    }

    #[test]
    fn the_ctrl_forms_still_work_while_searching() {
        // Which is what makes abandoning the search to reach an order unnecessary.
        let mut state = state();
        search(&mut state, "a");
        assert_eq!(names(&state)[0], "feat/live");

        ctrl(&mut state, 'o');
        assert!(state.is_filtering(), "still typing");
        assert_eq!(state.query(), "a", "and `o` did not join the query");
        assert_eq!(names(&state)[0], "main");
    }

    #[test]
    fn choosing_a_repository_hands_the_keyboard_to_the_branch_list() {
        let mut state = two_repos("/src/app");
        search(&mut state, "tools");
        assert!(state.is_filtering());
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.step(), Step::Branch);
        assert!(
            !state.is_filtering(),
            "a step arrives at its list, not in its search box"
        );
    }

    #[test]
    fn the_offer_to_create_sits_last_and_survives_a_partial_match() {
        // The offer cannot be conditional on an empty list: a query can match a branch and
        // still be a name worth creating.
        let mut state = state();
        search(&mut state, "dep");
        let rows = state.rows();
        assert_eq!(
            rows[0].name, "chore/deps",
            "the fuzzy match is still listed first"
        );
        assert_eq!(rows.last().unwrap().name, "dep");
        assert_eq!(rows.last().unwrap().state, BranchState::New);
    }

    #[test]
    fn n_starts_a_branch_from_the_row_under_the_cursor() {
        let mut state = state();
        let base = under_cursor(&state);

        state.handle_key(key(KeyCode::Char('n')));

        assert_eq!(state.step(), Step::Name);
        assert_eq!(state.naming(), Some((base.as_str(), "")));
    }

    #[test]
    fn the_base_does_not_move_while_the_name_is_being_typed() {
        let mut state = state();
        let base = under_cursor(&state);
        state.handle_key(key(KeyCode::Char('n')));

        type_in(&mut state, "feat/x");
        state.handle_key(key(KeyCode::Down));
        ctrl(&mut state, 'n');

        assert_eq!(state.naming(), Some((base.as_str(), "feat/x")));
    }

    #[test]
    fn enter_carries_the_name_and_the_base_into_the_destination_step() {
        let mut state = state();
        let base = under_cursor(&state);
        state.handle_key(key(KeyCode::Char('n')));
        type_in(&mut state, "feat/x");

        state.handle_key(key(KeyCode::Enter));

        assert_eq!(state.step(), Step::Destination);
        match state.chosen() {
            Some(Chosen::NewFrom { name, base: from }) => {
                assert_eq!(name, "feat/x");
                assert_eq!(from.name, base);
            }
            other => panic!("expected a new branch, got {other:?}"),
        }
    }

    #[test]
    fn esc_from_the_destination_comes_back_to_the_name_still_typed() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('n')));
        type_in(&mut state, "feat/x");
        state.handle_key(key(KeyCode::Enter));

        state.handle_key(key(KeyCode::Esc));

        assert_eq!(state.step(), Step::Name);
        assert_eq!(state.naming().map(|(_, name)| name), Some("feat/x"));
    }

    #[test]
    fn esc_from_the_name_goes_back_to_the_list_and_forgets_it() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('n')));
        type_in(&mut state, "feat/x");

        state.handle_key(key(KeyCode::Esc));

        assert_eq!(state.step(), Step::Branch);
        assert_eq!(state.naming(), None);
    }

    #[test]
    fn ctrl_u_clears_the_name_without_leaving_the_step() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('n')));
        type_in(&mut state, "feat/x");

        ctrl(&mut state, 'u');

        assert_eq!(state.step(), Step::Name);
        assert_eq!(state.naming().map(|(_, name)| name), Some(""));
    }

    #[test]
    fn a_name_that_is_empty_invalid_or_taken_is_refused_at_the_prompt() {
        for bad in ["", "  ", "feat/..x", "main"] {
            let mut state = state();
            state.handle_key(key(KeyCode::Char('n')));
            type_in(&mut state, bad);

            state.handle_key(key(KeyCode::Enter));

            assert_eq!(state.step(), Step::Name, "{bad:?} should not have advanced");
            assert!(state.message().is_some(), "{bad:?} should say why not");
        }
    }

    #[test]
    fn n_is_text_while_the_search_box_has_the_keyboard() {
        let mut state = state();
        search(&mut state, "feat/brand-new");
        assert_eq!(state.rows()[state.cursor()].state, BranchState::New);

        state.handle_key(key(KeyCode::Char('n')));

        assert_eq!(state.step(), Step::Branch);
        assert_eq!(state.query(), "feat/brand-newn");
    }

    #[test]
    fn a_name_that_matches_nothing_becomes_an_offer_to_create_it() {
        let mut state = state();
        search(&mut state, "feat/brand-new");
        assert_eq!(names(&state), ["feat/brand-new"]);
        assert_eq!(state.rows()[0].state, BranchState::New);
    }

    #[test]
    fn a_query_git_would_reject_is_not_offered_as_a_new_branch() {
        for bad in [
            "feat//x",
            "feat..x",
            "has space",
            "-dashed",
            "x.lock",
            "a:b",
            "@",
        ] {
            let mut state = state();
            search(&mut state, bad);
            assert!(
                state.rows().iter().all(|e| e.state != BranchState::New),
                "{bad} should not be offered"
            );
        }
    }

    #[test]
    fn an_existing_branch_is_never_duplicated_by_the_create_offer() {
        let mut state = state();
        search(&mut state, "main");
        assert_eq!(names(&state), ["main"]);
        assert_ne!(state.rows()[0].state, BranchState::New);
    }

    #[test]
    fn picking_a_branch_that_is_already_running_jumps_instead_of_asking_where() {
        let mut state = state();
        search(&mut state, "feat/live");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            BranchAction::Jump {
                pane_id: "w2:p1".into()
            }
        );
        assert_eq!(state.step(), Step::Branch, "no destination step is entered");
    }

    #[test]
    fn picking_any_other_branch_moves_to_the_destination_step() {
        let mut state = state();
        search(&mut state, "chore");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            BranchAction::Consumed
        );
        assert_eq!(state.step(), Step::Destination);
        assert_eq!(state.chosen().unwrap().name(), "chore/deps");
    }

    #[test]
    fn enter_enter_takes_the_first_destination_which_is_split_here() {
        let mut state = state();
        search(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        let action = state.handle_key(key(KeyCode::Enter));
        let BranchAction::Chosen(choice) = action else {
            panic!("expected a choice, got {action:?}");
        };
        assert_eq!(choice.chosen.name(), "chore/deps");
        assert_eq!(choice.destination, destinations()[0]);
    }

    #[test]
    fn escape_backs_out_of_the_destination_step_rather_than_quitting() {
        let mut state = state();
        search(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Consumed);
        assert_eq!(state.step(), Step::Branch);
        assert!(state.chosen().is_none());
        // A second Esc, with only one repository, is at the top level and does quit.
        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Quit);
    }

    #[test]
    fn tab_goes_back_to_the_panes_view() {
        assert_eq!(
            state().handle_key(key(KeyCode::Tab)),
            BranchAction::ShowPanes
        );
        assert_eq!(
            two_repos("/src/app").handle_key(key(KeyCode::Tab)),
            BranchAction::ShowPanes,
            "including from the repository step"
        );
    }

    #[test]
    fn the_destination_cursor_wraps() {
        let mut state = state();
        search(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        for _ in 0..destinations().len() {
            state.handle_key(key(KeyCode::Down));
        }
        assert_eq!(state.destination_cursor(), 0);
        state.handle_key(key(KeyCode::Up));
        assert_eq!(state.destination_cursor(), destinations().len() - 1);
    }

    // --- the repository step ---------------------------------------------------------

    #[test]
    fn choosing_a_repository_asks_for_its_branches_and_lets_go_of_the_last_ones() {
        let mut state = two_repos("/src/app");
        state.set_data(app_branches());
        assert_eq!(names(&state), ["feat/live", "main", "chore/deps"]);

        state.handle_key(key(KeyCode::Down));
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            BranchAction::LoadRepo {
                repo_root: "/src/tools".into()
            }
        );
        assert_eq!(state.step(), Step::Branch);
        assert_eq!(state.repo().display_name, "me/tools");
        assert_eq!(
                names(&state),
                ["main"],
                "only what the new repository itself says: its open checkout, and nothing of the one just left"
            );
        assert!(state.is_loading());
    }

    #[test]
    fn escape_goes_back_to_the_repository_list_when_there_is_one_to_go_back_to() {
        let mut state = two_repos("/src/app");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.step(), Step::Branch);
        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Consumed);
        assert_eq!(state.step(), Step::Repo);
        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Quit);
    }

    #[test]
    fn typing_narrows_the_repository_list_by_name_or_by_path() {
        let mut state = two_repos("/src/app");
        search(&mut state, "tools");
        assert_eq!(repo_names(&state), ["me/tools"]);

        ctrl(&mut state, 'u');
        assert_eq!(repo_names(&state).len(), 2);

        search(&mut state, "src/app");
        assert_eq!(repo_names(&state), ["me/app"]);
    }

    #[test]
    fn a_query_that_matches_no_repository_refuses_rather_than_opening_the_wrong_one() {
        let mut state = two_repos("/src/app");
        search(&mut state, "nothing-like-this");
        assert!(state.repo_rows().is_empty());
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            BranchAction::Consumed
        );
        assert_eq!(state.step(), Step::Repo);
        assert!(state.message().is_some());
    }

    // --- ordering --------------------------------------------------------------------

    #[test]
    fn ctrl_f_asks_for_the_repository_on_screen_to_be_fetched() {
        let mut state = state();
        assert_eq!(
            ctrl(&mut state, 'f'),
            BranchAction::Fetch {
                repo_root: "/src/app".into()
            }
        );

        // The caller answers by handing back data that says so, which is what the prompt
        // line reads to say `fetching origin…`.
        assert!(!state.is_fetching());
        state.set_data(BranchData {
            fetching: true,
            ..app_branches()
        });
        assert!(state.is_fetching());
    }

    #[test]
    fn fetching_is_only_offered_where_there_is_a_repository_on_screen() {
        // On the repository step there is a list of them and no one is chosen yet; on the
        // destination step the question has already moved on.
        let mut on_the_repo_step = two_repos("/src/app");
        assert_eq!(on_the_repo_step.step(), Step::Repo);
        assert_eq!(ctrl(&mut on_the_repo_step, 'f'), BranchAction::Ignored);

        let mut state = state();
        search(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.step(), Step::Destination);
        assert_eq!(ctrl(&mut state, 'f'), BranchAction::Ignored);
    }

    #[test]
    fn ctrl_u_empties_the_search_the_way_the_key_hint_says_it_does() {
        let mut state = state();
        search(&mut state, "chore");
        assert_eq!(state.query(), "chore");
        assert_eq!(ctrl(&mut state, 'u'), BranchAction::Consumed);
        assert_eq!(state.query(), "");
        assert_eq!(names(&state).len(), 3);
    }
}
