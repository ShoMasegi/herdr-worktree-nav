//! The branches picker: choose a branch, then choose where its pane goes.
//!
//! Two steps rather than a key per destination, so the destinations can grow without the
//! keymap growing with them. The first destination is "split here" and it starts selected,
//! which makes Enter Enter the fast path.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::domain::dest::Destination;
use crate::domain::model::RepoNode;
use crate::domain::resolve::{self, BranchEntry, BranchState};
use crate::port::{GitRef, PullRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchAction {
    Consumed,
    Ignored,
    Quit,
    /// `Tab` — back to the panes view.
    ShowPanes,
    /// The branch is already being worked on; go there instead of checking it out again.
    Jump {
        pane_id: String,
    },
    /// Boxed: this variant is far larger than the others, and the enum is returned from
    /// every keystroke.
    Chosen(Box<Choice>),
}

/// A branch and where its pane should go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub entry: BranchEntry,
    pub destination: Destination,
}

/// Which of the two steps is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Branch,
    Destination,
}

pub struct BranchesState {
    repo: RepoNode,
    local_refs: Vec<GitRef>,
    remote_heads: Vec<String>,
    pull_requests: Vec<PullRequest>,
    /// Every branch, before filtering.
    entries: Vec<BranchEntry>,
    /// Indices into `entries`, in display order.
    visible: Vec<usize>,
    /// Offered when the query matches nothing: create a branch by that name.
    proposed: Option<BranchEntry>,
    cursor: usize,
    query: String,
    step: Step,
    destinations: Vec<Destination>,
    destination_cursor: usize,
    chosen: Option<BranchEntry>,
    /// The remote listing is still in flight, so the list may still grow.
    loading: bool,
    message: Option<String>,
}

impl BranchesState {
    pub fn new(repo: RepoNode, local_refs: Vec<GitRef>, destinations: Vec<Destination>) -> Self {
        let mut state = Self {
            repo,
            local_refs,
            remote_heads: Vec::new(),
            pull_requests: Vec::new(),
            entries: Vec::new(),
            visible: Vec::new(),
            proposed: None,
            cursor: 0,
            query: String::new(),
            step: Step::Branch,
            destinations,
            destination_cursor: 0,
            chosen: None,
            loading: true,
            message: None,
        };
        state.reresolve();
        state
    }

    /// Fold in the remote listing once it arrives. The cursor stays where it was, because
    /// the user may already be typing.
    pub fn set_remote_heads(&mut self, heads: Vec<String>) {
        self.remote_heads = heads;
        self.loading = false;
        self.reresolve();
    }

    pub fn set_pull_requests(&mut self, pull_requests: Vec<PullRequest>) {
        self.pull_requests = pull_requests;
        self.reresolve();
    }

    /// The remote listing finished without producing anything — offline, or no remote.
    pub fn finish_loading(&mut self) {
        self.loading = false;
    }

    pub fn repo(&self) -> &RepoNode {
        &self.repo
    }

    pub fn step(&self) -> Step {
        self.step
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn destination_cursor(&self) -> usize {
        self.destination_cursor
    }

    pub fn destinations(&self) -> &[Destination] {
        &self.destinations
    }

    /// The branch whose destination is being chosen.
    pub fn chosen(&self) -> Option<&BranchEntry> {
        self.chosen.as_ref()
    }

    /// The branches on screen, in order.
    pub fn rows(&self) -> Vec<&BranchEntry> {
        let mut rows: Vec<&BranchEntry> = self.visible.iter().map(|i| &self.entries[*i]).collect();
        if let Some(proposed) = &self.proposed {
            rows.push(proposed);
        }
        rows
    }

    /// The breadcrumb under the list: where this branch is, and what picking it will do.
    pub fn detail(&self) -> String {
        let Some(entry) = self.selected() else {
            return String::new();
        };
        let mut parts = vec![self.repo.display_name.clone(), entry.name.clone()];
        match &entry.state {
            BranchState::LivePane {
                pane_id,
                checkout_path,
            } => {
                parts.push(format!("open in {pane_id}"));
                parts.push(checkout_path.clone());
            }
            BranchState::IdleWorktree { checkout_path } => {
                parts.push("checked out, nothing running".to_string());
                parts.push(checkout_path.clone());
            }
            BranchState::LocalRef => parts.push("local branch, no worktree yet".to_string()),
            BranchState::RemoteOnly => {
                parts.push("on the remote, never fetched".to_string());
            }
            BranchState::New => parts.push("does not exist yet".to_string()),
        }
        if let Some(pr) = &entry.pull_request {
            parts.push(format!(
                "#{} {}{}",
                pr.number,
                pr.title,
                if pr.is_draft { " (draft)" } else { "" }
            ));
        }
        parts.join(" \u{b7} ")
    }

    /// The breadcrumb for the destination step: what the highlighted choice will do.
    pub fn destination_detail(&self) -> String {
        let Some(destination) = self.destinations.get(self.destination_cursor) else {
            return String::new();
        };
        let branch = self
            .chosen
            .as_ref()
            .map(|entry| entry.name.as_str())
            .unwrap_or("the branch");
        match destination {
            Destination::SplitHere { direction, .. } => format!(
                "{branch} opens beside the pane you came from, split {}",
                direction.as_str()
            ),
            Destination::ExistingTab { label, .. } => {
                format!("{branch} opens as a new pane in {label}")
            }
            Destination::ExistingSpace { workspace_id, .. } => {
                format!("{branch} opens as a new tab in {workspace_id}")
            }
            Destination::NewSpace => {
                format!("{branch} opens in a space of its own, as herdr would")
            }
        }
    }

    fn selected(&self) -> Option<&BranchEntry> {
        self.rows().get(self.cursor).copied()
    }

    fn reresolve(&mut self) {
        let anchor = self.selected().map(|entry| entry.name.clone());
        self.entries = resolve::resolve(
            &self.repo,
            &self.local_refs,
            &self.remote_heads,
            &self.pull_requests,
        );
        self.refilter();
        if let Some(name) = anchor {
            if let Some(index) = self.rows().iter().position(|entry| entry.name == name) {
                self.cursor = index;
            }
        }
    }

    fn refilter(&mut self) {
        let query = self.query.trim();
        self.visible = if query.is_empty() {
            (0..self.entries.len()).collect()
        } else {
            let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
            let mut matcher = Matcher::new(Config::DEFAULT);
            let mut buf = Vec::new();
            let mut scored: Vec<(u32, usize)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    let haystack = match &entry.pull_request {
                        Some(pr) => format!("{} #{} {}", entry.name, pr.number, pr.title),
                        None => entry.name.clone(),
                    };
                    pattern
                        .score(Utf32Str::new(&haystack, &mut buf), &mut matcher)
                        .map(|score| (score, index))
                })
                .collect();
            // Best match first; ties keep the resolve order, which is work-in-progress first.
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            scored.into_iter().map(|(_, index)| index).collect()
        };

        // A name that matches nothing is an offer to create it, not an empty list. Only
        // when it is a plausible branch name, and never when it already exists.
        self.proposed = (!query.is_empty()
            && is_branch_name(query)
            && !self.entries.iter().any(|entry| entry.name == query))
        .then(|| resolve::new_branch(query));

        self.cursor = 0;
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = match self.step {
            Step::Branch => self.rows().len(),
            Step::Destination => self.destinations.len(),
        };
        if len == 0 {
            return;
        }
        let cursor = match self.step {
            Step::Branch => &mut self.cursor,
            Step::Destination => &mut self.destination_cursor,
        };
        *cursor = (*cursor as isize + delta).rem_euclid(len as isize) as usize;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> BranchAction {
        if key.kind == KeyEventKind::Release {
            return BranchAction::Ignored;
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
                _ => BranchAction::Ignored,
            };
        }

        match self.step {
            Step::Branch => self.handle_branch_key(key),
            Step::Destination => self.handle_destination_key(key),
        }
    }

    fn handle_branch_key(&mut self, key: KeyEvent) -> BranchAction {
        match key.code {
            KeyCode::Esc => BranchAction::Quit,
            KeyCode::Tab => BranchAction::ShowPanes,
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
            KeyCode::Enter => {
                let Some(entry) = self.selected().cloned() else {
                    self.message = Some("no branch selected".into());
                    return BranchAction::Consumed;
                };
                // Already being worked on: go there. Asking where to put a second copy of
                // work that is already open would be the wrong question.
                if let BranchState::LivePane { pane_id, .. } = &entry.state {
                    return BranchAction::Jump {
                        pane_id: pane_id.clone(),
                    };
                }
                self.chosen = Some(entry);
                self.step = Step::Destination;
                self.destination_cursor = 0;
                BranchAction::Consumed
            }
            // The branch list is a search box: letters are text, not commands. There is no
            // mode to enter, because typing a branch name is the common case.
            KeyCode::Char(c) => {
                self.query.push(c);
                self.refilter();
                BranchAction::Consumed
            }
            _ => BranchAction::Ignored,
        }
    }

    fn handle_destination_key(&mut self, key: KeyEvent) -> BranchAction {
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => {
                self.step = Step::Branch;
                self.chosen = None;
                BranchAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                BranchAction::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                BranchAction::Consumed
            }
            KeyCode::Enter => {
                let (Some(entry), Some(destination)) = (
                    self.chosen.clone(),
                    self.destinations.get(self.destination_cursor).cloned(),
                ) else {
                    self.message = Some("no destination available".into());
                    return BranchAction::Consumed;
                };
                BranchAction::Chosen(Box::new(Choice { entry, destination }))
            }
            _ => BranchAction::Ignored,
        }
    }
}

/// Whether a typed string could be a branch name, so that a stray query does not turn into
/// an offer to create something git would refuse anyway.
fn is_branch_name(query: &str) -> bool {
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
    use crate::domain::model::{PaneNode, WorktreeNode};
    use crate::port::{AgentStatus, RefKind, SplitDirection};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_in(state: &mut BranchesState, text: &str) {
        for c in text.chars() {
            state.handle_key(key(KeyCode::Char(c)));
        }
    }

    fn local(name: &str, at: i64) -> GitRef {
        GitRef {
            name: name.into(),
            kind: RefKind::Local,
            committed_at: Some(at),
            subject: Some(format!("work on {name}")),
        }
    }

    fn repo() -> RepoNode {
        RepoNode {
            repo_key: "/src/app/.git".into(),
            repo_root: "/src/app".into(),
            display_name: "me/app".into(),
            worktrees: vec![WorktreeNode {
                branch: Some("feat/live".into()),
                checkout_path: "/wt/feat-live".into(),
                is_primary: false,
                open_workspace_id: Some("w2".into()),
                panes: vec![PaneNode {
                    pane_id: "w2:p1".into(),
                    workspace_id: "w2".into(),
                    tab_id: "w2:t1".into(),
                    display_name: Some("claude".into()),
                    agent_status: AgentStatus::Idle,
                    focused: false,
                }],
            }],
        }
    }

    fn destinations() -> Vec<Destination> {
        vec![
            Destination::SplitHere {
                tab_id: "w1:t1".into(),
                target_pane_id: "w1:p1".into(),
                direction: SplitDirection::Right,
            },
            Destination::ExistingSpace {
                workspace_id: "w3".into(),
                label: "w3 \u{2192} new tab".into(),
            },
            Destination::NewSpace,
        ]
    }

    fn state() -> BranchesState {
        BranchesState::new(
            repo(),
            vec![
                local("feat/live", 10),
                local("main", 20),
                local("chore/deps", 5),
            ],
            destinations(),
        )
    }

    fn names(state: &BranchesState) -> Vec<String> {
        state.rows().iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn typing_filters_immediately_with_no_mode_to_enter() {
        let mut state = state();
        type_in(&mut state, "chore");
        assert_eq!(state.query(), "chore");
        assert_eq!(names(&state)[0], "chore/deps");
    }

    #[test]
    fn the_offer_to_create_sits_last_and_survives_a_partial_match() {
        // Typing `feat/login-v2` while `feat/login` exists must still offer to create it,
        // so the offer cannot be conditional on the list being empty. It goes last so it
        // never gets in the way of an existing branch.
        let mut state = state();
        type_in(&mut state, "dep");
        let rows = state.rows();
        assert_eq!(
            rows[0].name, "chore/deps",
            "the fuzzy match is still listed first"
        );
        assert_eq!(rows.last().unwrap().name, "dep");
        assert_eq!(rows.last().unwrap().state, BranchState::New);
    }

    #[test]
    fn a_name_that_matches_nothing_becomes_an_offer_to_create_it() {
        let mut state = state();
        type_in(&mut state, "feat/brand-new");
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
            type_in(&mut state, bad);
            assert!(
                state.rows().iter().all(|e| e.state != BranchState::New),
                "{bad} should not be offered"
            );
        }
    }

    #[test]
    fn an_existing_branch_is_never_duplicated_by_the_create_offer() {
        let mut state = state();
        type_in(&mut state, "main");
        assert_eq!(names(&state), ["main"]);
        assert_ne!(state.rows()[0].state, BranchState::New);
    }

    #[test]
    fn picking_a_branch_that_is_already_running_jumps_instead_of_asking_where() {
        let mut state = state();
        type_in(&mut state, "feat/live");
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
        type_in(&mut state, "chore");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            BranchAction::Consumed
        );
        assert_eq!(state.step(), Step::Destination);
        assert_eq!(state.chosen().unwrap().name, "chore/deps");
    }

    #[test]
    fn enter_enter_takes_the_first_destination_which_is_split_here() {
        let mut state = state();
        type_in(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        let action = state.handle_key(key(KeyCode::Enter));
        let BranchAction::Chosen(choice) = action else {
            panic!("expected a choice, got {action:?}");
        };
        assert_eq!(choice.entry.name, "chore/deps");
        assert_eq!(choice.destination, destinations()[0]);
    }

    #[test]
    fn escape_backs_out_of_the_destination_step_rather_than_quitting() {
        let mut state = state();
        type_in(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Consumed);
        assert_eq!(state.step(), Step::Branch);
        assert!(state.chosen().is_none());
        // A second Esc, now at the top level, does quit.
        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Quit);
    }

    #[test]
    fn the_remote_listing_folds_in_without_moving_the_cursor_off_what_was_selected() {
        let mut state = state();
        assert!(state.is_loading());
        type_in(&mut state, "chore");
        let before = state.rows()[state.cursor()].name.clone();

        state.set_remote_heads(vec!["chore/deps".into(), "feat/from-the-remote".into()]);
        assert!(!state.is_loading());
        assert_eq!(state.rows()[state.cursor()].name, before);

        // The new branch is there once the filter allows it.
        for _ in 0..5 {
            state.handle_key(key(KeyCode::Backspace));
        }
        assert!(names(&state).contains(&"feat/from-the-remote".to_string()));
    }

    #[test]
    fn pull_requests_arrive_late_and_only_annotate() {
        let mut state = state();
        let before = names(&state);
        state.set_pull_requests(vec![PullRequest {
            number: 7,
            title: "Bump deps".into(),
            head_ref: "chore/deps".into(),
            is_draft: false,
        }]);
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
        state.set_pull_requests(vec![PullRequest {
            number: 123,
            title: "Bump dependencies".into(),
            head_ref: "chore/deps".into(),
            is_draft: false,
        }]);
        type_in(&mut state, "123");
        assert_eq!(names(&state)[0], "chore/deps");
    }

    #[test]
    fn tab_goes_back_to_the_panes_view() {
        assert_eq!(
            state().handle_key(key(KeyCode::Tab)),
            BranchAction::ShowPanes
        );
    }

    #[test]
    fn the_destination_cursor_wraps() {
        let mut state = state();
        type_in(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        for _ in 0..destinations().len() {
            state.handle_key(key(KeyCode::Down));
        }
        assert_eq!(state.destination_cursor(), 0);
        state.handle_key(key(KeyCode::Up));
        assert_eq!(state.destination_cursor(), destinations().len() - 1);
    }
}
