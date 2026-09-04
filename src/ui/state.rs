//! Picker state and key handling.
//!
//! Key handling is pure — it maps a key and the current state to an [`Action`] — so the
//! whole keymap is covered by ordinary tests.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::domain::model::{PaneNode, Tree};
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
    /// Show the branches picker. `None` when the cursor is not in a repository — the
    /// picker opens on its list of them either way.
    ShowBranches {
        repo_root: Option<String>,
    },
    /// Delete a checkout. The only thing this plugin does that cannot be undone by doing it
    /// again the other way.
    ///
    /// `panes` are closed first, in the order given. A checkout with panes in it is the
    /// ordinary end state of a finished worktree, not an unusual one — see
    /// `docs/adr/0010-closing-the-panes-first.md`.
    RemoveWorktree {
        repo_root: String,
        checkout_path: String,
        label: String,
        panes: Vec<String>,
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
    /// A removal waiting on a yes. Nothing on disk has been touched yet.
    pending_removal: Option<Removal>,
    message: Option<String>,
    /// Frame of the spinner on the rows being removed. Advanced by the loop that owns the
    /// clock, the same way the branches view does it — `domain` is not allowed to read one.
    tick: usize,
    /// Whether an answer is still on its way, which the prompt line says with a spinner.
    waiting: bool,
    /// Checkouts git has answered about, dirty or not. Not drawn; see `set_answered`.
    answered: Vec<String>,
}

/// A checkout the user has asked to delete, held until they say yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub repo_root: String,
    pub checkout_path: String,
    /// The branch name, for the question and for saying what went.
    pub label: String,
    /// The panes that stop if this goes ahead, in the order the tree lists them. Named in
    /// the question because uncommitted work is git's to protect and this is not: whatever
    /// a working agent has in flight has no other safety net.
    pub panes: Vec<PaneNode>,
}

impl PanesState {
    /// `home` shortens checkout paths to `~/...`; pass `None` to leave them absolute.
    pub fn new(tree: Tree, home: Option<String>) -> Self {
        let mut state = Self {
            tree,
            options: ViewOptions {
                home,
                ..ViewOptions::default()
            },
            rows: Vec::new(),
            lines: Vec::new(),
            cursor: 0,
            filtering: false,
            pending_removal: None,
            message: None,
            tick: 0,
            waiting: false,
            answered: Vec::new(),
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

    /// Say which checkouts are holding uncommitted work. Arrives after the first frame, one
    /// answer at a time, so nothing may move under the reader: the cursor stays where it is,
    /// the row count cannot change, and the meta column is measured with room for these
    /// already kept (`domain::rows::marks_reserve`).
    pub fn set_dirty(&mut self, paths: Vec<String>) {
        if self.options.dirty == paths {
            return;
        }
        self.options.dirty = paths;
        // The cursor is not touched at all, which says the promise above more strongly than
        // clamping it would: `dirty` feeds nothing but `Row::is_dirty`, so the row list that
        // comes back has the same length and the same order it went in with.
        self.rows = rows::flatten(&self.tree, &self.options);
        self.lines = rows::display_lines(&self.rows);
    }

    /// Whether something is still being waited for, which the prompt line turns a spinner
    /// for. Set by the loop; the state cannot see a thread any more than it can see a clock.
    pub fn set_waiting(&mut self, waiting: bool) {
        self.waiting = waiting;
    }

    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    /// Say which checkouts git has answered for at all, whatever it said. Nothing is drawn
    /// from this: it is what stops "nobody has asked yet" being read as "clean" when the
    /// question is whether somebody's panes may be closed.
    pub fn set_answered(&mut self, paths: Vec<String>) {
        self.answered = paths;
    }

    /// Say which checkouts git would not answer for, so their rows can say it themselves. A
    /// row with no marker is then the absence of a claim rather than a claim of clean.
    pub fn set_unreadable(&mut self, paths: Vec<String>) {
        if self.options.unreadable == paths {
            return;
        }
        self.options.unreadable = paths;
        self.rows = rows::flatten(&self.tree, &self.options);
        self.lines = rows::display_lines(&self.rows);
    }

    /// Say which checkouts are being removed, so their rows can say so and stop being
    /// selectable. The removals themselves are running in other processes entirely.
    ///
    /// The cursor holds its place rather than being sent back to the top: tidying up comes
    /// in batches, and the next thing to delete is usually the next row down. It only moves
    /// when the row it is on has just become one of these.
    pub fn set_removing(&mut self, paths: Vec<String>) {
        if self.options.removing == paths {
            return;
        }
        self.options.removing = paths;
        let at = self.cursor;
        self.rows = rows::flatten(&self.tree, &self.options);
        self.lines = rows::display_lines(&self.rows);
        self.cursor =
            rows::next_row(&self.rows, &self.lines, at.min(self.lines.len())).unwrap_or(0);
    }

    /// Advance the spinner one frame. Called by the loop that owns the clock, and only
    /// while something is actually being removed.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    /// Which frame of the spinner the rows being removed should show.
    pub fn frame(&self) -> usize {
        self.tick
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

    /// Shortens checkout paths to `~`, for anything drawn outside the rows.
    pub fn home(&self) -> Option<&str> {
        self.options.home.as_deref()
    }

    /// The removal being asked about, which the picker turns into a dialog.
    pub fn pending_removal(&self) -> Option<&Removal> {
        self.pending_removal.as_ref()
    }

    /// Say something in the search line until the next key. git says its piece over several
    /// lines and this is one, so the whitespace is collapsed on the way in.
    pub fn set_message(&mut self, message: String) {
        self.message = Some(message.split_whitespace().collect::<Vec<_>>().join(" "));
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
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
        let at = self.cursor;
        self.rows = rows::flatten(&self.tree, &self.options);
        self.lines = rows::display_lines(&self.rows);
        // From where the cursor was rather than from the top. A reload that lands you back
        // at the first row is a reload that costs you your place, and the row that just went
        // is where the next thing to tidy up usually is — tidying comes in batches.
        let from = at.min(self.lines.len().saturating_sub(1));
        self.cursor = rows::next_row(&self.rows, &self.lines, from).unwrap_or(0);
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
            rows::next_row(&self.rows, &self.lines, start)
        } else {
            rows::previous_row(&self.rows, &self.lines, start)
        }
        .unwrap_or(self.cursor);
    }

    /// Jump to the head of the previous or next group.
    fn move_group(&mut self, steps: isize) {
        if let Some(index) = rows::step_group(&self.rows, &self.lines, self.cursor, steps) {
            self.cursor = index;
        }
    }

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
        if self.options.removing.contains(&worktree.checkout_path) {
            // A second one would race the first, and would close panes that the first is
            // already having removed out from under them.
            self.message = Some("that checkout is already being removed".into());
            return Action::Consumed;
        }
        // The refusals below are only reached by a checkout with panes in it, and that is
        // the whole reason they exist: for an empty one there is nothing to lose by letting
        // git answer for itself, but here the panes are gone by the time it speaks.
        if !worktree.panes.is_empty() {
            if self.options.dirty.contains(&worktree.checkout_path) {
                self.message = Some("that checkout is holding work nobody has committed".into());
                return Action::Consumed;
            }
            if self.options.unreadable.contains(&worktree.checkout_path) {
                self.message = Some("git would not read that working tree".into());
                return Action::Consumed;
            }
            // Not asked yet is not the same as asked and clean, and only the second is a
            // licence to close somebody's panes. Walking a working tree takes a moment and
            // the answers land after the first frame, so this is the ordinary state of the
            // checkout the picker opens on — the one the cursor is already sitting in.
            if !self.answered.contains(&worktree.checkout_path) {
                self.message = Some("still reading that working tree — try again".into());
                return Action::Consumed;
            }
        }
        self.pending_removal = Some(Removal {
            repo_root: repo.repo_root.clone(),
            checkout_path: worktree.checkout_path.clone(),
            label: worktree.label().to_string(),
            panes: worktree.panes.clone(),
        });
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

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Windows sends both press and release; only act on press.
        if key.kind == KeyEventKind::Release {
            return Action::Ignored;
        }
        self.message = None;

        // A question is on screen and it is the only thing the keyboard is for.
        if let Some(removal) = self.pending_removal.take() {
            return match key.code {
                KeyCode::Char('y') => Action::RemoveWorktree {
                    repo_root: removal.repo_root,
                    checkout_path: removal.checkout_path,
                    label: removal.label,
                    panes: removal.panes.into_iter().map(|pane| pane.pane_id).collect(),
                },
                // Anything else is a no. Taking the removal above is what makes that true
                // of keys nobody thought of as well as of the ones they did.
                _ => Action::Consumed,
            };
        }

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
            KeyCode::Char('r') => Action::Reload,
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
            // A repository under the cursor is a preselection, not a requirement: the
            // branches picker starts by asking which repository anyway.
            KeyCode::Tab => Action::ShowBranches {
                repo_root: self
                    .selected_repo_index()
                    .map(|index| self.tree.repos[index].repo_root.clone()),
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
        PanesState::new(
            Tree {
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
                            track: None,
                            panes: vec![pane("w1:p1", "claude", AgentStatus::Working)],
                        },
                        WorktreeNode {
                            branch: Some("feat/login".into()),
                            checkout_path: "/wt/app/feat-login".into(),
                            is_primary: false,
                            open_workspace_id: Some("w2".into()),
                            track: None,
                            panes: vec![pane("w2:p1", "codex", AgentStatus::Blocked)],
                        },
                        WorktreeNode {
                            branch: Some("fix/crash".into()),
                            checkout_path: "/wt/app/fix-crash".into(),
                            is_primary: false,
                            open_workspace_id: None,
                            track: None,
                            panes: vec![],
                        },
                    ],
                }],
                ungrouped: vec![pane("w9:p1", "zsh", AgentStatus::Unknown)],
            },
            None,
        )
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
        // The cursor does not stop here — the panes listed under it are what you pick —
        // but the answer is still the work rather than a second copy of it.
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
        // Headings and the checkouts that already have panes under them are stepped over:
        // there is nowhere to go on either, and stopping would only lengthen the walk.
        let mut state = state();
        let first = state.selected().unwrap().label.clone();
        let mut stops = vec![first.clone()];
        for _ in 0..state.lines().len() {
            state.handle_key(key(KeyCode::Down));
            let label = state.selected().unwrap().label.clone();
            if label == first {
                break;
            }
            stops.push(label);
        }
        assert_eq!(stops, ["claude", "codex", "fix/crash", "zsh"]);

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
        assert_eq!(state.selected().unwrap().label, "claude");

        state.handle_key(key(KeyCode::Right));
        assert_eq!(
            state.selected().unwrap().label,
            "zsh",
            "the panes in no repository are a section like any other"
        );
        state.handle_key(key(KeyCode::Right));
        assert_eq!(state.selected().unwrap().label, "claude", "and it wraps");

        // From deep inside a group, one press still leaves it.
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected().unwrap().label, "fix/crash");
        state.handle_key(key(KeyCode::Left));
        assert_eq!(state.selected().unwrap().label, "zsh");
    }

    #[test]
    fn h_and_l_are_the_arrows_by_another_name() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('l')));
        assert_eq!(state.selected().unwrap().label, "zsh");
        state.handle_key(key(KeyCode::Char('h')));
        assert_eq!(state.selected().unwrap().label, "claude");
    }

    #[test]
    fn the_arrows_keep_working_while_the_search_box_has_focus() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        assert!(state.is_filtering());
        state.handle_key(key(KeyCode::Right));
        assert_eq!(state.selected().unwrap().label, "zsh");
        assert_eq!(state.query(), "", "an arrow is not text");

        // A letter is, though: `l` types rather than moves once the search box has focus.
        state.handle_key(key(KeyCode::Char('l')));
        assert_eq!(state.query(), "l");
    }

    #[test]
    fn moving_up_from_the_top_wraps_to_the_last_thing_worth_going_to() {
        let mut state = state();
        state.handle_key(key(KeyCode::Up));
        assert_eq!(state.selected().unwrap().label, "zsh");
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
        assert_eq!(asked.label, "fix/crash");
        assert_eq!(asked.checkout_path, "/wt/app/fix-crash");
        assert_eq!(asked.repo_root, "/src/app");

        assert_eq!(
            state.handle_key(key(KeyCode::Char('y'))),
            Action::RemoveWorktree {
                repo_root: "/src/app".into(),
                checkout_path: "/wt/app/fix-crash".into(),
                label: "fix/crash".into(),
                panes: Vec::new(),
            }
        );
        assert!(
            state.pending_removal().is_none(),
            "the question is answered"
        );
    }

    #[test]
    fn the_cursor_steps_off_a_checkout_once_its_removal_has_started() {
        // The removal is running in a process of its own by then, so there is nothing left
        // to do to the row: a second `Shift-D` would race the first, and `Enter` would open
        // a checkout being deleted underneath it.
        let mut state = state();
        select(&mut state, "fix/crash");
        state.set_removing(vec!["/wt/app/fix-crash".into()]);

        assert_ne!(
            state.selected().map(|row| row.label.as_str()),
            Some("fix/crash")
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Char('D'))),
            Action::Consumed,
            "and Shift-D cannot reach it"
        );
        assert!(state.pending_removal().is_none());
    }

    #[test]
    fn a_removal_that_finished_gives_the_row_back() {
        let mut state = state();
        state.set_removing(vec!["/wt/app/fix-crash".into()]);
        state.set_removing(Vec::new());
        select(&mut state, "fix/crash");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(
            state.pending_removal().is_some(),
            "a refused removal leaves a checkout that can be asked about again"
        );
    }

    #[test]
    fn anything_that_is_not_y_is_a_no() {
        // Including keys nobody thought of: the question is taken off the screen first and
        // only `y` puts a removal in its place.
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
        // A finished worktree has panes in it — that is its ordinary end state, not an
        // unusual one. The cursor cannot land on a checkout that has panes (its panes are
        // the answer to where to go), so the key has to mean this from the pane row.
        let mut state = state();
        // Two panes, so the assertion below can tell "the checkout's panes" from "the first
        // pane in the checkout", and can see them reordered.
        let mut tree = state.tree().clone();
        tree.repos[0].worktrees[1]
            .panes
            .push(pane("w2:p2", "zsh", AgentStatus::Unknown));
        state.replace_tree(tree);
        state.set_answered(vec!["/wt/app/feat-login".into()]);
        select(&mut state, "codex");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);

        let asked = state.pending_removal().expect("a question should be up");
        assert_eq!(asked.label, "feat/login");
        let closing: Vec<&str> = asked
            .panes
            .iter()
            .map(|pane| pane.pane_id.as_str())
            .collect();
        assert_eq!(closing, ["w2:p1", "w2:p2"], "and it names what stops");

        // And `y` carries them through, in the order the question listed them. Without this
        // the picker could ask about panes it then never closed, and remove the checkout out
        // from under every one of them.
        assert_eq!(
            state.handle_key(key(KeyCode::Char('y'))),
            Action::RemoveWorktree {
                repo_root: "/src/app".into(),
                checkout_path: "/wt/app/feat-login".into(),
                label: "feat/login".into(),
                panes: vec!["w2:p1".into(), "w2:p2".into()],
            }
        );
    }

    #[test]
    fn a_checkout_with_panes_whose_working_tree_has_not_answered_yet_is_refused() {
        // The state the picker opens in: the walk is behind the first frame, the cursor is
        // already on the pane the user came from, and nothing has answered. Reading that as
        // clean would close their panes on a guess — and `r` puts every checkout back into
        // it, so this is not only a startup window.
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
        // The rows of a checkout being removed stop being selectable, but its panes' rows do
        // not, and a second confirmation would close panes the first is already removing the
        // ground from under.
        let mut state = state();
        state.set_answered(vec!["/wt/app/feat-login".into()]);
        state.set_removing(vec!["/wt/app/feat-login".into()]);
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
        // The refusal exists here and not on an empty checkout because of what is at stake:
        // for an empty one git can answer for itself, but here the panes would already be
        // closed by the time it did.
        let mut state = state();
        state.set_dirty(vec!["/wt/app/feat-login".into()]);
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
        state.set_unreadable(vec!["/wt/app/feat-login".into()]);
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
        state.set_dirty(vec!["/wt/app/fix-crash".into()]);
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

        // The repository's own checkout, with nothing running in it, so the cursor can
        // reach it: git cannot remove a main working tree, and it is not a worktree.
        let mut on_the_repo = PanesState::new(
            Tree {
                repos: vec![RepoNode {
                    repo_key: "/src/app/.git".into(),
                    repo_root: "/src/app".into(),
                    display_name: "me/app".into(),
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
    fn panes_that_are_not_in_a_repository_are_always_listed() {
        // They are still panes. A picker that hides some of them makes you wonder which.
        assert!(row_labels(&state()).contains(&"zsh".to_string()));
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
