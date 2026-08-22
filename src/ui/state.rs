//! Picker state and key handling.
//!
//! Key handling is pure — it maps a key and the current state to an [`Action`] — so the
//! whole keymap is covered by ordinary tests.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::domain::model::Tree;
use crate::domain::rows::{self, DisplayLine, Row, RowRef, StateFilter, ViewOptions};

/// What the event loop should do about a key. Anything that touches herdr is returned
/// rather than performed, so the terminal can be restored first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The key was consumed and only the display changed.
    Consumed,
    /// The key meant nothing here.
    Ignored,
    Quit,
    /// Focus this pane.
    Jump(String),
    /// Open a checkout that no workspace currently has open.
    OpenWorktree {
        repo_root: String,
        checkout_path: String,
    },
    /// Add a pane to a checkout that is already open.
    NewPane {
        checkout_path: String,
        beside_pane_id: String,
    },
    /// Show the branches of this repository.
    ShowBranches {
        repo_root: String,
    },
    Reload,
}

pub struct PanesState {
    tree: Tree,
    options: ViewOptions,
    rows: Vec<Row>,
    /// Rows interleaved with the blank lines that separate groups. The cursor indexes this,
    /// not `rows`, so scrolling and drawing agree on what line 5 is.
    lines: Vec<DisplayLine>,
    cursor: usize,
    /// `/` filter mode: typing edits the query instead of running commands.
    filtering: bool,
    message: Option<String>,
}

impl PanesState {
    pub fn new(tree: Tree) -> Self {
        let mut state = Self {
            tree,
            options: ViewOptions::default(),
            rows: Vec::new(),
            lines: Vec::new(),
            cursor: 0,
            filtering: false,
            message: None,
        };
        state.rebuild(None);
        state
    }

    /// Replace the tree after a reload, keeping the cursor on the same pane when it is
    /// still there.
    pub fn replace_tree(&mut self, tree: Tree) {
        let anchor = self.selected_pane_id().map(str::to_string);
        self.tree = tree;
        self.rebuild(anchor.as_deref());
    }

    /// Put the cursor on a specific pane, used to open the picker on the pane you came from.
    pub fn focus_pane(&mut self, pane_id: &str) {
        if let Some(index) = self.line_of_pane(pane_id) {
            self.cursor = index;
        }
    }

    fn line_of_pane(&self, pane_id: &str) -> Option<usize> {
        self.lines.iter().position(|line| match line {
            DisplayLine::Spacer => false,
            DisplayLine::Row(index) => match self.rows[*index].reference {
                RowRef::Pane(r, w, p) => {
                    self.tree.repos[r].worktrees[w].panes[p].pane_id == pane_id
                }
                RowRef::Ungrouped(p) => self.tree.ungrouped[p].pane_id == pane_id,
                _ => false,
            },
        })
    }

    pub fn lines(&self) -> &[DisplayLine] {
        &self.lines
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Index into [`lines`](Self::lines), not into `rows`.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn query(&self) -> &str {
        &self.options.query
    }

    pub fn state_filter(&self) -> Option<StateFilter> {
        self.options.state_filter
    }

    pub fn is_filtering(&self) -> bool {
        self.filtering
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn showing_ungrouped(&self) -> bool {
        self.options.show_ungrouped
    }

    /// Every pane in the session, for the count beside the search box.
    pub fn pane_count(&self) -> usize {
        let grouped: usize = self
            .tree
            .repos
            .iter()
            .flat_map(|repo| repo.worktrees.iter())
            .map(|worktree| worktree.panes.len())
            .sum();
        grouped + self.tree.ungrouped.len()
    }

    /// The breadcrumb for the row under the cursor.
    pub fn detail(&self) -> String {
        match self.selected() {
            Some(row) => rows::detail(&self.tree, row.reference),
            None => String::new(),
        }
    }

    fn rebuild(&mut self, anchor: Option<&str>) {
        self.rows = rows::flatten(&self.tree, &self.options);
        self.lines = rows::display_lines(&self.rows);
        self.cursor = rows::next_row(&self.lines, 0).unwrap_or(0);
        if let Some(pane_id) = anchor {
            self.focus_pane(pane_id);
        }
    }

    fn selected(&self) -> Option<&Row> {
        match self.lines.get(self.cursor)? {
            DisplayLine::Spacer => None,
            DisplayLine::Row(index) => self.rows.get(*index),
        }
    }

    fn selected_pane_id(&self) -> Option<&str> {
        match self.selected()?.reference {
            RowRef::Pane(r, w, p) => {
                Some(self.tree.repos[r].worktrees[w].panes[p].pane_id.as_str())
            }
            RowRef::Ungrouped(p) => Some(self.tree.ungrouped[p].pane_id.as_str()),
            _ => None,
        }
    }

    /// The repository the cursor is inside, whatever kind of row it is on.
    fn selected_repo_index(&self) -> Option<usize> {
        match self.selected()?.reference {
            RowRef::Repo(r) | RowRef::Worktree(r, _) | RowRef::Pane(r, _, _) => Some(r),
            _ => None,
        }
    }

    /// The worktree the cursor is inside, whether it is on the worktree row or on one of
    /// its panes.
    fn selected_worktree(&self) -> Option<(usize, usize)> {
        match self.selected()?.reference {
            RowRef::Worktree(r, w) | RowRef::Pane(r, w, _) => Some((r, w)),
            _ => None,
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.lines.is_empty() {
            return;
        }
        let len = self.lines.len();
        let start = (self.cursor as isize + delta).rem_euclid(len as isize) as usize;
        self.cursor = if delta >= 0 {
            rows::next_row(&self.lines, start)
        } else {
            rows::previous_row(&self.lines, start)
        }
        .unwrap_or(self.cursor);
    }

    fn toggle_collapse(&mut self, repo_index: usize) {
        let key = self.tree.repos[repo_index].repo_key.clone();
        if !self.options.collapsed.remove(&key) {
            self.options.collapsed.insert(key);
        }
        self.rows = rows::flatten(&self.tree, &self.options);
        self.lines = rows::display_lines(&self.rows);
        // Keep the cursor on the repository the user just folded rather than jumping.
        self.cursor = self
            .lines
            .iter()
            .position(|line| match line {
                DisplayLine::Row(index) => self.rows[*index].reference == RowRef::Repo(repo_index),
                DisplayLine::Spacer => false,
            })
            .unwrap_or(0);
    }

    /// What Enter means on the current row.
    fn activate(&mut self) -> Action {
        let Some(row) = self.selected() else {
            return Action::Consumed;
        };
        match row.reference {
            RowRef::Repo(repo_index) => {
                self.toggle_collapse(repo_index);
                Action::Consumed
            }
            RowRef::Pane(r, w, p) => {
                Action::Jump(self.tree.repos[r].worktrees[w].panes[p].pane_id.clone())
            }
            RowRef::Ungrouped(p) => Action::Jump(self.tree.ungrouped[p].pane_id.clone()),
            RowRef::Worktree(r, w) => {
                let repo = &self.tree.repos[r];
                let worktree = &repo.worktrees[w];
                match worktree.panes.first() {
                    // A checkout that is already being worked in: go to the work rather
                    // than opening a second copy of it.
                    Some(pane) => Action::Jump(pane.pane_id.clone()),
                    None => Action::OpenWorktree {
                        repo_root: repo.repo_root.clone(),
                        checkout_path: worktree.checkout_path.clone(),
                    },
                }
            }
            RowRef::UngroupedRepo => Action::Consumed,
        }
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

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Windows sends both press and release; only act on press.
        if key.kind == KeyEventKind::Release {
            return Action::Ignored;
        }
        self.message = None;

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
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
            };
        }

        if self.filtering {
            return self.handle_filter_key(key);
        }

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
            KeyCode::Enter => self.activate(),
            KeyCode::Char('n') => self.new_pane(),
            KeyCode::Char('r') => Action::Reload,
            KeyCode::Char('b') => self.set_state_filter(Some(StateFilter::Blocked)),
            KeyCode::Char('w') => self.set_state_filter(Some(StateFilter::Working)),
            KeyCode::Char('i') => self.set_state_filter(Some(StateFilter::Idle)),
            KeyCode::Char('d') => self.set_state_filter(Some(StateFilter::Done)),
            KeyCode::Char('a') => self.set_state_filter(None),
            KeyCode::Char('h') => {
                self.options.show_ungrouped = !self.options.show_ungrouped;
                let anchor = self.selected_pane_id().map(str::to_string);
                self.rebuild(anchor.as_deref());
                Action::Consumed
            }
            KeyCode::Char('/') => {
                self.filtering = true;
                Action::Consumed
            }
            KeyCode::Tab => match self.selected_repo_index() {
                Some(index) => Action::ShowBranches {
                    repo_root: self.tree.repos[index].repo_root.clone(),
                },
                None => {
                    self.message = Some("select a repository first".into());
                    Action::Consumed
                }
            },
            _ => Action::Ignored,
        }
    }

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
    use crate::domain::model::{PaneNode, RepoNode, WorktreeNode};
    use crate::port::AgentStatus;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn pane(id: &str, name: &str, status: AgentStatus) -> PaneNode {
        let workspace = id.split(':').next().unwrap().to_string();
        PaneNode {
            pane_id: id.into(),
            tab_id: format!("{workspace}:t1"),
            workspace_id: workspace,
            display_name: Some(name.into()),
            agent_status: status,
            focused: false,
        }
    }

    /// `me/app` with main (a working agent), feat/login (blocked), and an idle fix/crash.
    fn state() -> PanesState {
        PanesState::new(Tree {
            repos: vec![RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                worktrees: vec![
                    WorktreeNode {
                        branch: Some("main".into()),
                        checkout_path: "/src/app".into(),
                        is_primary: true,
                        open_workspace_id: Some("w1".into()),
                        panes: vec![pane("w1:p1", "claude", AgentStatus::Working)],
                    },
                    WorktreeNode {
                        branch: Some("feat/login".into()),
                        checkout_path: "/wt/app/feat-login".into(),
                        is_primary: false,
                        open_workspace_id: Some("w2".into()),
                        panes: vec![pane("w2:p1", "codex", AgentStatus::Blocked)],
                    },
                    WorktreeNode {
                        branch: Some("fix/crash".into()),
                        checkout_path: "/wt/app/fix-crash".into(),
                        is_primary: false,
                        open_workspace_id: None,
                        panes: vec![],
                    },
                ],
            }],
            ungrouped: vec![pane("w9:p1", "zsh", AgentStatus::Unknown)],
        })
    }

    /// Put the cursor on the row with this label.
    fn select(state: &mut PanesState, label: &str) {
        let index = state
            .rows()
            .iter()
            .position(|row| row.label == label)
            .unwrap_or_else(|| panic!("no row labelled {label}"));
        state.cursor = state
            .lines()
            .iter()
            .position(|line| *line == DisplayLine::Row(index))
            .unwrap();
    }

    fn row_labels(state: &PanesState) -> Vec<String> {
        state.rows().iter().map(|r| r.label.clone()).collect()
    }

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
    fn enter_on_a_repository_folds_it_and_leaves_the_cursor_there() {
        let mut state = state();
        select(&mut state, "me/app (2)");
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Consumed);
        assert_eq!(row_labels(&state), ["me/app (2)"]);
        assert!(!state.rows()[0].expanded);
        assert_eq!(state.cursor(), 0);

        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Consumed);
        assert!(state.rows()[0].expanded);
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
    fn tab_asks_for_the_branches_of_the_repository_the_cursor_is_in() {
        let mut state = state();
        select(&mut state, "codex");
        assert_eq!(
            state.handle_key(key(KeyCode::Tab)),
            Action::ShowBranches {
                repo_root: "/src/app".into()
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
    fn h_toggles_panes_that_are_not_in_a_repository() {
        let mut state = state();
        assert!(!state.showing_ungrouped());
        assert!(!row_labels(&state).contains(&"zsh".to_string()));

        state.handle_key(key(KeyCode::Char('h')));
        assert!(state.showing_ungrouped());
        assert!(row_labels(&state).contains(&"zsh".to_string()));
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
    fn reloading_keeps_the_cursor_on_the_same_pane() {
        let mut state = state();
        select(&mut state, "codex");
        let before = state.cursor();
        let tree = state.tree().clone();
        state.replace_tree(tree);
        assert_eq!(state.cursor(), before);
    }

    #[test]
    fn the_breadcrumb_follows_the_cursor() {
        let mut state = state();
        select(&mut state, "fix/crash");
        assert_eq!(state.detail(), "me/app · fix/crash · /wt/app/fix-crash");
    }

    #[test]
    fn the_count_beside_the_search_box_includes_panes_outside_a_repository() {
        assert_eq!(state().pane_count(), 3);
    }

    #[test]
    fn a_key_release_is_ignored_so_windows_does_not_act_twice() {
        let mut state = state();
        let release =
            KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Release);
        assert_eq!(state.handle_key(release), Action::Ignored);
    }
}
