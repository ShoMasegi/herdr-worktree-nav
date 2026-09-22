//! Which rows the picker shows, in what order, and where the cursor lands after each
//! change to the list.

use crate::domain::model::{normalize_path, RepoNode};
use crate::domain::order::Order;
use crate::domain::resolve::{self, BranchEntry};
use crate::ui::branches::*;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

impl BranchesState {
    /// The repositories on screen, in order.
    pub fn repo_rows(&self) -> Vec<RepoRow<'_>> {
        self.repo_visible
            .iter()
            .map(|index| RepoRow {
                repo: &self.repos[*index],
                is_origin: Some(*index) == self.origin,
            })
            .collect()
    }

    /// The branches on screen, in order.
    pub fn rows(&self) -> Vec<&BranchEntry> {
        let mut rows: Vec<&BranchEntry> = self.visible.iter().map(|i| &self.entries[*i]).collect();
        if let Some(proposed) = &self.proposed {
            // Last whatever the order is: it is an offer, not one of the repository's
            // branches, and it must never push a real one out of the way.
            rows.push(proposed);
        }
        rows
    }

    pub(super) fn selected(&self) -> Option<&BranchEntry> {
        self.rows().get(self.cursor).copied()
    }

    pub(super) fn reresolve(&mut self) {
        let anchor = self.selected().map(|entry| entry.name.clone());
        self.entries = resolve::resolve(
            self.repo(),
            &self.data.local_refs,
            &self.data.remote_heads,
            &self.data.pull_requests,
        );
        self.refilter();
        self.restore_cursor(anchor);
    }

    /// Put the cursor back on a named branch after the list has been rebuilt.
    fn restore_cursor(&mut self, anchor: Option<String>) {
        let Some(name) = anchor else {
            return;
        };
        if let Some(index) = self.rows().iter().position(|entry| entry.name == name) {
            self.cursor = index;
        }
    }

    pub(super) fn refilter(&mut self) {
        let query = self.query.trim();
        let mut matched: Vec<usize> = if query.is_empty() {
            (0..self.entries.len()).collect()
        } else {
            let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
            let mut matcher = Matcher::new(Config::DEFAULT);
            let mut buf = Vec::new();
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    let haystack = match &entry.pull_request {
                        Some(pr) => format!("{} #{} {}", entry.name, pr.number, pr.title),
                        None => entry.name.clone(),
                    };
                    pattern
                        .score(Utf32Str::new(&haystack, &mut buf), &mut matcher)
                        .is_some()
                })
                .map(|(index, _)| index)
                .collect()
        };

        // The fuzzy score decides what is in the list; the chosen order decides where it
        // sits. Sorting by score instead would silently override an order the user picked,
        // the moment they typed anything.
        let (order, entries) = (self.order, &self.entries);
        matched.sort_by(|a, b| order.compare(&entries[*a], &entries[*b]));
        self.visible = matched;

        // A name that matches nothing is an offer to create it, not an empty list. Only
        // when it is a plausible branch name, and never when it already exists.
        self.proposed = (!query.is_empty()
            && is_branch_name(query)
            && !self.entries.iter().any(|entry| entry.name == query))
        .then(|| resolve::new_branch(query));

        self.cursor = 0;
    }

    pub(super) fn refilter_repos(&mut self) {
        let query = self.repo_query.trim();
        self.repo_visible = if query.is_empty() {
            (0..self.repos.len()).collect()
        } else {
            let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
            let mut matcher = Matcher::new(Config::DEFAULT);
            let mut buf = Vec::new();
            self.repos
                .iter()
                .enumerate()
                .filter(|(_, repo)| {
                    // The path is searchable too: two checkouts of the same fork are told
                    // apart by where they are, not by what they are called.
                    let haystack = format!("{} {}", repo.display_name, repo.repo_root);
                    pattern
                        .score(Utf32Str::new(&haystack, &mut buf), &mut matcher)
                        .is_some()
                })
                .map(|(index, _)| index)
                .collect()
        };
        self.repo_cursor = 0;
    }

    /// Reorder and go to the top.
    ///
    /// What a new order is for is seeing what is now first. Following the branch under the
    /// cursor would leave it wherever that row happened to land.
    pub(super) fn reorder(&mut self, order: Order) {
        self.order = order;
        self.refilter();
    }

    pub(super) fn move_cursor(&mut self, delta: isize) {
        let len = match self.step {
            Step::Repo => self.repo_visible.len(),
            Step::Branch => self.rows().len(),
            Step::Destination => self.destinations.len(),
            // The list is on screen but frozen: the base was chosen before the name was
            // asked for, and moving off it would silently change what is being cut from.
            Step::Name => return,
        };
        if len == 0 {
            return;
        }
        let cursor = match self.step {
            Step::Repo => &mut self.repo_cursor,
            Step::Branch => &mut self.cursor,
            Step::Destination => &mut self.destination_cursor,
            Step::Name => return,
        };
        *cursor = (*cursor as isize + delta).rem_euclid(len as isize) as usize;
    }
}

/// Which repository a path belongs to.
///
/// The path the picker is handed is whatever herdr knew about where the user was: a
/// repository root when the workspace is one herdr made for a worktree, and otherwise the
/// checkout the pane's working directory is in — which for a linked worktree is not the
/// repository root at all. Both have to land on the same row.
pub(crate) fn locate(repos: &[RepoNode], path: Option<&str>) -> Option<usize> {
    let path = normalize_path(path?);
    if path.is_empty() {
        return None;
    }
    repos
        .iter()
        .position(|repo| normalize_path(&repo.repo_root) == path)
        .or_else(|| {
            repos.iter().position(|repo| {
                repo.worktrees
                    .iter()
                    .any(|worktree| normalize_path(&worktree.checkout_path) == path)
            })
        })
}

/// Whether a typed string could be a branch name, so that a stray query does not turn into
/// an offer to create something git would refuse anyway.
pub(super) fn is_branch_name(query: &str) -> bool {
    !query.is_empty()
        && !query.starts_with('-')
        && !query.starts_with('/')
        && !query.ends_with('/')
        && !query.ends_with(".lock")
        && !query.contains("..")
        && !query.contains("//")
        && !query.contains('@')
        && !query
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::branches::fixtures::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::domain::order::SortKey;

    #[test]
    fn the_remote_listing_folds_in_without_moving_the_cursor_off_what_was_selected() {
        let mut state = state();
        assert!(state.is_loading());
        search(&mut state, "chore");
        let before = state.rows()[state.cursor()].name.clone();

        state.set_data(BranchData {
            remote_heads: vec!["chore/deps".into(), "feat/from-the-remote".into()],
            loading: false,
            ..app_branches()
        });
        assert!(!state.is_loading());
        assert_eq!(state.rows()[state.cursor()].name, before);

        for _ in 0..5 {
            state.handle_key(key(KeyCode::Backspace));
        }
        assert!(names(&state).contains(&"feat/from-the-remote".to_string()));
    }

    #[test]
    fn pull_requests_arrive_late_and_only_annotate() {
        let mut state = state();
        let before = names(&state);
        state.set_data(BranchData {
            pull_requests: vec![PullRequest {
                number: 7,
                title: "Bump deps".into(),
                head_ref: "chore/deps".into(),
                is_draft: false,
            }],
            ..app_branches()
        });
        assert_eq!(names(&state), before, "no rows are added or removed");
        let entry = state
            .rows()
            .into_iter()
            .find(|e| e.name == "chore/deps")
            .unwrap();
        assert_eq!(entry.pull_request.as_ref().unwrap().number, 7);
    }

    #[test]
    fn a_pull_request_can_be_searched_for_by_number_or_title() {
        let mut state = state();
        state.set_data(BranchData {
            pull_requests: vec![PullRequest {
                number: 123,
                title: "Bump dependencies".into(),
                head_ref: "chore/deps".into(),
                is_draft: false,
            }],
            ..app_branches()
        });
        search(&mut state, "123");
        assert_eq!(names(&state)[0], "chore/deps");
    }

    #[test]
    fn one_repository_is_nothing_to_choose_between() {
        let state = state();
        assert_eq!(state.step(), Step::Branch);
        assert!(!state.has_repo_step());
    }

    #[test]
    fn the_repository_step_starts_on_the_one_the_picker_was_summoned_from() {
        let state = two_repos("/src/tools");
        assert_eq!(state.step(), Step::Repo);
        assert!(state.has_repo_step());
        assert_eq!(repo_names(&state), ["me/app", "me/tools"], "name order");
        assert_eq!(state.repo_cursor(), 1);
        assert_eq!(state.repo().display_name, "me/tools");
        assert!(state.repo_rows()[1].is_origin);
        assert!(!state.repo_rows()[0].is_origin);
    }

    #[test]
    fn a_worktree_checkout_still_finds_the_repository_it_belongs_to() {
        let state = two_repos("/wt/feat-live");
        assert_eq!(state.repo().display_name, "me/app");
        assert_eq!(state.repo_cursor(), 0);
    }

    #[test]
    fn an_unknown_origin_simply_starts_at_the_top() {
        let state = two_repos("/somewhere/else");
        assert_eq!(state.repo_cursor(), 0);
        assert!(state.repo_rows().iter().all(|row| !row.is_origin));
    }

    #[test]
    fn ctrl_o_walks_the_orders_and_ctrl_r_turns_the_list_around() {
        let mut state = state();
        assert_eq!(
            names(&state),
            ["feat/live", "main", "chore/deps"],
            "by state: what is running, then the most recent"
        );

        ctrl(&mut state, 'o');
        assert_eq!(state.order().key, SortKey::Updated);
        assert_eq!(names(&state), ["main", "feat/live", "chore/deps"]);

        ctrl(&mut state, 'r');
        assert!(state.order().reversed);
        assert_eq!(names(&state), ["chore/deps", "feat/live", "main"]);

        ctrl(&mut state, 'o');
        assert_eq!(state.order().key, SortKey::Name);
        assert!(
            !state.order().reversed,
            "a new key comes back at its own natural direction"
        );
        assert_eq!(names(&state), ["chore/deps", "feat/live", "main"]);

        ctrl(&mut state, 'o');
        assert_eq!(state.order().key, SortKey::State, "and back round");
    }

    #[test]
    fn reordering_goes_to_the_top_rather_than_following_the_row_it_was_on() {
        let mut state = state();
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.rows()[state.cursor()].name, "chore/deps");

        state.handle_key(key(KeyCode::Char('i')));
        assert_eq!(state.cursor(), 0);
        assert_eq!(state.rows()[0].name, "main", "the most recently committed");

        state.handle_key(key(KeyCode::Down));
        state.handle_key(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT));
        assert_eq!(state.cursor(), 0);
        assert_eq!(state.rows()[0].name, "chore/deps", "the least");

        // The chords do the same thing, so they land in the same place.
        state.handle_key(key(KeyCode::Down));
        ctrl(&mut state, 'r');
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn new_data_arriving_still_leaves_the_cursor_where_it_was() {
        let mut state = state();
        state.handle_key(key(KeyCode::Down));
        let before = state.rows()[state.cursor()].name.clone();
        state.set_data(BranchData {
            remote_heads: vec!["feat/from-the-remote".into()],
            loading: false,
            ..app_branches()
        });
        assert_eq!(state.rows()[state.cursor()].name, before);
    }

    #[test]
    fn the_chosen_order_outranks_the_fuzzy_score() {
        let mut state = state();
        search(&mut state, "a");
        assert_eq!(names(&state)[0], "feat/live", "by state, it is running");

        ctrl(&mut state, 'o');
        assert_eq!(names(&state)[0], "main", "by date, it is the newer");
    }

    #[test]
    fn the_order_survives_switching_repository() {
        let mut state = two_repos("/src/app");
        state.handle_key(key(KeyCode::Enter));
        state.set_data(app_branches());
        ctrl(&mut state, 'o');
        ctrl(&mut state, 'r');
        let order = state.order();

        state.handle_key(key(KeyCode::Esc));
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.order(), order);
    }

    // --- working and failing ---------------------------------------------------------
}
