//! The branches picker: choose a repository, then a branch, then where its pane goes.
//!
//! Three steps rather than a key per destination, so the destinations can grow without the
//! keymap growing with them. The first destination is "split here" and it starts selected,
//! which makes Enter Enter the fast path once a branch is chosen.
//!
//! The repository step is skipped when herdr has only one repository open: a picker that
//! asks you to choose between one thing is asking nothing.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::domain::dest::Destination;
use crate::domain::model::{normalize_path, RepoNode};
use crate::domain::order::Order;
use crate::domain::preview::{self, Preview};
use crate::domain::resolve::{self, BranchEntry, BranchState};
use crate::port::{GitRef, PullRequest, Snapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchAction {
    Consumed,
    Ignored,
    Quit,
    /// `Tab` — back to the panes view.
    ShowPanes,
    /// A repository was chosen and its branches are not on screen yet. The caller reads
    /// them — from its cache or from git — and hands them back before the next frame.
    LoadRepo {
        repo_root: String,
    },
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

/// Which of the three steps is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Repo,
    Branch,
    Destination,
}

/// Everything known about one repository's branches. The picker holds one of these for the
/// repository on screen; the caller caches one per repository so going back and forth does
/// not re-run git.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchData {
    pub local_refs: Vec<GitRef>,
    pub remote_heads: Vec<String>,
    pub pull_requests: Vec<PullRequest>,
    /// The remote listing is still in flight, so the list may still grow.
    pub loading: bool,
}

/// One row of the repository step.
pub struct RepoRow<'a> {
    pub repo: &'a RepoNode,
    /// The repository the picker was summoned from.
    pub is_origin: bool,
}

pub struct BranchesState {
    /// Every repository herdr has open, in name order.
    repos: Vec<RepoNode>,
    /// The one the picker was summoned from, marked in the list.
    origin: Option<usize>,
    /// The one whose branches are on screen.
    current: usize,
    /// Indices into `repos`, in display order.
    repo_visible: Vec<usize>,
    repo_cursor: usize,
    repo_query: String,
    /// With one repository there is nothing to choose, so there is no step to go back to.
    has_repo_step: bool,
    /// Shortens checkout paths to `~`; `None` leaves them absolute.
    home: Option<String>,

    /// Kept so the destination step can show what each choice will do to the tab.
    snapshot: Snapshot,
    /// The current repository's branches, as far as they have been read.
    data: BranchData,
    /// Every branch, before filtering.
    entries: Vec<BranchEntry>,
    /// Indices into `entries`, in display order.
    visible: Vec<usize>,
    /// Offered when the query matches nothing: create a branch by that name.
    proposed: Option<BranchEntry>,
    cursor: usize,
    query: String,
    order: Order,

    step: Step,
    destinations: Vec<Destination>,
    destination_cursor: usize,
    chosen: Option<BranchEntry>,
    message: Option<String>,
}

impl BranchesState {
    /// `from` is the repository root or checkout the picker was summoned from, which decides
    /// where the cursor starts. The branches themselves are not read here: the caller loads
    /// the current repository's and hands them to [`BranchesState::set_data`].
    pub fn new(
        mut repos: Vec<RepoNode>,
        from: Option<&str>,
        destinations: Vec<Destination>,
        snapshot: Snapshot,
        home: Option<String>,
    ) -> Self {
        // Name order, fixed. A repository list that reshuffled between invocations would
        // make muscle memory worthless, and there is no recency to sort by.
        repos.sort_by(|a, b| {
            a.display_name
                .cmp(&b.display_name)
                .then_with(|| a.repo_root.cmp(&b.repo_root))
        });
        let origin = locate(&repos, from);
        let has_repo_step = repos.len() > 1;

        let mut state = Self {
            current: origin.unwrap_or(0),
            origin,
            repo_visible: (0..repos.len()).collect(),
            repos,
            repo_cursor: 0,
            repo_query: String::new(),
            has_repo_step,
            home,
            snapshot,
            data: BranchData {
                loading: true,
                ..BranchData::default()
            },
            entries: Vec::new(),
            visible: Vec::new(),
            proposed: None,
            cursor: 0,
            query: String::new(),
            order: Order::default(),
            step: if has_repo_step {
                Step::Repo
            } else {
                Step::Branch
            },
            destinations,
            destination_cursor: 0,
            chosen: None,
            message: None,
        };
        state.repo_cursor = state
            .repo_visible
            .iter()
            .position(|index| *index == state.current)
            .unwrap_or(0);
        state.reresolve();
        state
    }

    /// Hand the current repository everything read for it so far. The cursor stays on the
    /// branch it was on, because the remote listing may land while the user is already
    /// typing.
    pub fn set_data(&mut self, data: BranchData) {
        self.data = data;
        self.reresolve();
    }

    pub fn repo(&self) -> &RepoNode {
        &self.repos[self.current]
    }

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

    pub fn step(&self) -> Step {
        self.step
    }

    /// Whether Esc has a step to go back to rather than closing the picker.
    pub fn has_repo_step(&self) -> bool {
        self.has_repo_step
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn repo_query(&self) -> &str {
        &self.repo_query
    }

    pub fn order(&self) -> Order {
        self.order
    }

    pub fn home(&self) -> Option<&str> {
        self.home.as_deref()
    }

    pub fn is_loading(&self) -> bool {
        self.data.loading
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn repo_cursor(&self) -> usize {
        self.repo_cursor
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
            // Last whatever the order is: it is an offer, not one of the repository's
            // branches, and it must never push a real one out of the way.
            rows.push(proposed);
        }
        rows
    }

    /// The breadcrumb under the repository list: the full path of the row under the cursor.
    pub fn repo_detail(&self) -> String {
        let Some(row) = self.repo_visible.get(self.repo_cursor) else {
            return String::new();
        };
        let repo = &self.repos[*row];
        format!("{} \u{b7} {}", repo.display_name, repo.repo_root)
    }

    /// The breadcrumb under the list: where this branch is, and what picking it will do.
    pub fn detail(&self) -> String {
        let Some(entry) = self.selected() else {
            return String::new();
        };
        let mut parts = vec![self.repo().display_name.clone(), entry.name.clone()];
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

    /// What the tab under the cursor will look like once the branch's pane lands in it.
    pub fn preview(&self) -> Preview {
        let (Some(destination), Some(entry)) = (
            self.destinations.get(self.destination_cursor),
            self.chosen.as_ref(),
        ) else {
            return Preview::Unavailable;
        };
        preview::predict(&self.snapshot, destination, &entry.name)
    }

    /// Whether the destination under the cursor can actually take the pane.
    fn destination_is_blocked(&self) -> Option<String> {
        match self.preview() {
            Preview::Blocked { reason, .. } => Some(reason),
            _ => None,
        }
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

    fn refilter(&mut self) {
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

    fn refilter_repos(&mut self) {
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

    /// Reorder without losing the branch the cursor is on.
    fn reorder(&mut self, order: Order) {
        self.order = order;
        let anchor = self.selected().map(|entry| entry.name.clone());
        self.refilter();
        self.restore_cursor(anchor);
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = match self.step {
            Step::Repo => self.repo_visible.len(),
            Step::Branch => self.rows().len(),
            Step::Destination => self.destinations.len(),
        };
        if len == 0 {
            return;
        }
        let cursor = match self.step {
            Step::Repo => &mut self.repo_cursor,
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
                        Step::Destination => return BranchAction::Ignored,
                    }
                    BranchAction::Consumed
                }
                // The list is a search box, so the order cannot be a letter: `o` and `r`
                // are branch names being typed.
                KeyCode::Char('o') if self.step == Step::Branch => {
                    self.reorder(self.order.cycle());
                    BranchAction::Consumed
                }
                KeyCode::Char('r') if self.step == Step::Branch => {
                    self.reorder(self.order.reverse());
                    BranchAction::Consumed
                }
                _ => BranchAction::Ignored,
            };
        }

        match self.step {
            Step::Repo => self.handle_repo_key(key),
            Step::Branch => self.handle_branch_key(key),
            Step::Destination => self.handle_destination_key(key),
        }
    }

    fn handle_repo_key(&mut self, key: KeyEvent) -> BranchAction {
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
                self.repo_query.pop();
                self.refilter_repos();
                BranchAction::Consumed
            }
            KeyCode::Enter => {
                let Some(index) = self.repo_visible.get(self.repo_cursor).copied() else {
                    self.message = Some("no repository selected".into());
                    return BranchAction::Consumed;
                };
                self.open_repo(index)
            }
            // Same bargain as the branch list: letters are text, not commands.
            KeyCode::Char(c) => {
                self.repo_query.push(c);
                self.refilter_repos();
                BranchAction::Consumed
            }
            _ => BranchAction::Ignored,
        }
    }

    /// Move to a repository's branches. Whatever was on screen belonged to the repository
    /// being left, so it goes with it; the caller fills the gap before the next frame.
    fn open_repo(&mut self, index: usize) -> BranchAction {
        self.current = index;
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
            KeyCode::Esc => {
                if self.has_repo_step {
                    self.step = Step::Repo;
                    BranchAction::Consumed
                } else {
                    BranchAction::Quit
                }
            }
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
                // Acting would look like it worked and quietly do nothing, so say why not.
                if let Some(reason) = self.destination_is_blocked() {
                    self.message = Some(reason);
                    return BranchAction::Consumed;
                }
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
    use crate::domain::order::SortKey;
    use crate::port::{AgentStatus, RefKind, SplitDirection};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(state: &mut BranchesState, c: char) -> BranchAction {
        state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
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

    fn other_repo() -> RepoNode {
        RepoNode {
            repo_key: "/src/tools/.git".into(),
            repo_root: "/src/tools".into(),
            display_name: "me/tools".into(),
            worktrees: vec![WorktreeNode {
                branch: Some("main".into()),
                checkout_path: "/src/tools".into(),
                is_primary: true,
                open_workspace_id: Some("w5".into()),
                panes: vec![],
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

    /// `w1:t1` holds one pane; `w3:t1` is zoomed, which herdr refuses to move a pane into.
    fn snapshot() -> Snapshot {
        serde_json::from_value(serde_json::json!({
            "version": "0.7.4",
            "protocol": 16,
            "workspaces": [],
            "tabs": [
                {"tab_id": "w1:t1", "workspace_id": "w1", "label": "agents", "number": 1,
                 "focused": true, "pane_count": 1, "agent_status": "idle"},
                {"tab_id": "w3:t1", "workspace_id": "w3", "label": "zoomed", "number": 1,
                 "focused": false, "pane_count": 1, "agent_status": "idle"}
            ],
            "panes": [
                {"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1",
                 "terminal_id": "t1", "focused": true, "agent": "claude",
                 "agent_status": "idle"}
            ],
            "layouts": [
                {"tab_id": "w1:t1", "workspace_id": "w1", "zoomed": false,
                 "area": {"x": 0, "y": 0, "width": 100, "height": 40},
                 "focused_pane_id": "w1:p1",
                 "panes": [{"pane_id": "w1:p1", "focused": true,
                            "rect": {"x": 0, "y": 0, "width": 100, "height": 40}}]},
                {"tab_id": "w3:t1", "workspace_id": "w3", "zoomed": true,
                 "area": {"x": 0, "y": 0, "width": 100, "height": 40},
                 "focused_pane_id": "w3:p1",
                 "panes": [{"pane_id": "w3:p1", "focused": true,
                            "rect": {"x": 0, "y": 0, "width": 100, "height": 40}}]}
            ]
        }))
        .expect("snapshot fixture should deserialize")
    }

    /// What git found for `me/app`, with the remote listing still in flight.
    fn app_branches() -> BranchData {
        BranchData {
            local_refs: vec![
                local("feat/live", 10),
                local("main", 20),
                local("chore/deps", 5),
            ],
            loading: true,
            ..BranchData::default()
        }
    }

    /// One repository open, so the picker starts on its branches.
    fn state() -> BranchesState {
        let mut state = BranchesState::new(
            vec![repo()],
            Some("/src/app"),
            destinations(),
            snapshot(),
            None,
        );
        state.set_data(app_branches());
        state
    }

    /// Two repositories open, so the picker starts on the repository step. Passed in
    /// reverse to prove the list is sorted rather than taken as given.
    fn two_repos(from: &str) -> BranchesState {
        BranchesState::new(
            vec![other_repo(), repo()],
            Some(from),
            destinations(),
            snapshot(),
            None,
        )
    }

    fn names(state: &BranchesState) -> Vec<String> {
        state.rows().iter().map(|e| e.name.clone()).collect()
    }

    fn repo_names(state: &BranchesState) -> Vec<String> {
        state
            .repo_rows()
            .iter()
            .map(|row| row.repo.display_name.clone())
            .collect()
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
        // A second Esc, with only one repository, is at the top level and does quit.
        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Quit);
    }

    #[test]
    fn the_remote_listing_folds_in_without_moving_the_cursor_off_what_was_selected() {
        let mut state = state();
        assert!(state.is_loading());
        type_in(&mut state, "chore");
        let before = state.rows()[state.cursor()].name.clone();

        state.set_data(BranchData {
            remote_heads: vec!["chore/deps".into(), "feat/from-the-remote".into()],
            loading: false,
            ..app_branches()
        });
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
        type_in(&mut state, "123");
        assert_eq!(names(&state)[0], "chore/deps");
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
    fn a_zoomed_destination_says_why_instead_of_quietly_doing_nothing() {
        // herdr answers a move into a zoomed tab with success and then does not move it, so
        // the picker has to stop before asking.
        let mut state = BranchesState::new(
            vec![repo()],
            Some("/src/app"),
            vec![Destination::ExistingTab {
                tab_id: "w3:t1".into(),
                label: "w3  zoomed".into(),
            }],
            snapshot(),
            None,
        );
        state.set_data(BranchData {
            local_refs: vec![local("chore/deps", 5)],
            ..BranchData::default()
        });
        type_in(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.step(), Step::Destination);

        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            BranchAction::Consumed
        );
        assert!(
            state.message().unwrap_or_default().contains("zoomed"),
            "got {:?}",
            state.message()
        );
        assert_eq!(state.step(), Step::Destination, "still asking");
    }

    #[test]
    fn the_preview_follows_the_destination_cursor() {
        let mut state = state();
        type_in(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        // The first destination splits w1:t1, which holds one pane.
        let Preview::Layout { panes, .. } = state.preview() else {
            panic!("expected a layout, got {:?}", state.preview());
        };
        assert_eq!(panes.len(), 2);
        assert!(panes.iter().any(|p| p.is_new && p.label == "chore/deps"));
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

    // --- the repository step ---------------------------------------------------------

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
        // The picker is handed wherever the user was, which for a pane in a linked worktree
        // is the checkout, not the repository root. Matching only the root would drop the
        // cursor on whatever happens to be first.
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
        type_in(&mut state, "tools");
        assert_eq!(repo_names(&state), ["me/tools"]);

        ctrl(&mut state, 'u');
        assert_eq!(repo_names(&state).len(), 2);

        // Two checkouts of one fork are told apart by where they are, not by their name.
        type_in(&mut state, "src/app");
        assert_eq!(repo_names(&state), ["me/app"]);
    }

    #[test]
    fn a_query_that_matches_no_repository_refuses_rather_than_opening_the_wrong_one() {
        let mut state = two_repos("/src/app");
        type_in(&mut state, "nothing-like-this");
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
    fn reordering_keeps_the_cursor_on_the_branch_it_was_on() {
        let mut state = state();
        state.handle_key(key(KeyCode::Down));
        let before = state.rows()[state.cursor()].name.clone();
        assert_eq!(before, "main");

        ctrl(&mut state, 'o');
        assert_eq!(state.rows()[state.cursor()].name, before);
        assert_eq!(state.cursor(), 0, "which has moved to the top");
    }

    #[test]
    fn the_chosen_order_outranks_the_fuzzy_score() {
        // Sorting the filtered list by score would quietly override the order the user
        // picked, the moment they typed anything.
        let mut state = state();
        type_in(&mut state, "a");
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

    #[test]
    fn ctrl_u_empties_the_search_the_way_the_key_hint_says_it_does() {
        let mut state = state();
        type_in(&mut state, "chore");
        assert_eq!(state.query(), "chore");
        assert_eq!(ctrl(&mut state, 'u'), BranchAction::Consumed);
        assert_eq!(state.query(), "");
        assert_eq!(names(&state).len(), 3);
    }
}
