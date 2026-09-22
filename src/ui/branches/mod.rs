//! The branches picker: choose a repository, then a branch, then where its pane goes.
//!
//! Three steps rather than a key per destination, so the destinations can grow without the
//! keymap growing with them. The first destination is "split here" and it starts selected,
//! which makes Enter Enter the fast path once a branch is chosen.
//!
//! The repository step is skipped when herdr has only one repository open: a picker that
//! asks you to choose between one thing is asking nothing.
//!
//! The state and what it answers about itself are here. The three things it does with that
//! state are a module each: [`list`] builds the rows and moves the cursor through them,
//! [`detail`] is every sentence the picker puts under the list, and [`keys`] is the keymap,
//! which is a step machine and reads as one.

pub(crate) mod detail;
mod keys;
pub(crate) mod list;

#[cfg(test)]
pub(crate) mod fixtures;

pub(crate) use list::locate;

use crate::domain::dest::Destination;
use crate::domain::model::RepoNode;
use crate::domain::order::Order;
use crate::domain::progress::Stage;
use crate::domain::resolve::{BranchEntry, Chosen};
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
    /// Bring this repository up to date with its remote.
    Fetch {
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
    pub chosen: Chosen,
    pub destination: Destination,
}

/// Which of the steps is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Repo,
    Branch,
    /// Typing the name of a branch to cut from the one under the cursor. A step of its own
    /// rather than a mode of the branch list, so that `Esc` from the destination comes back
    /// to the name rather than throwing it away.
    Name,
    Destination,
}

/// A branch being started from another one, while its name is being typed.
#[derive(Debug, Clone)]
struct Naming {
    base: BranchEntry,
    name: String,
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
    /// A `git fetch` of the whole repository is running, asked for by the user.
    pub fetching: bool,
}

/// What the picker is doing, once the choosing is over.
///
/// The list stays on screen throughout: the highlighted destination is the one being acted
/// on, and the preview beside it is the tab being built. Only the prompt line and the key
/// hint change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    /// Still a picker.
    Choosing,
    /// A step is in flight.
    Working { stage: Stage },
    /// It failed, and the screen is being held so the reason can be read. Without this the
    /// popup would vanish the instant the process ended, which is indistinguishable from
    /// having worked.
    Failed { stage: Stage, error: String },
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

    /// `/` search mode: typing edits the query instead of running commands. Both lists
    /// have one, and it is the same flag — only one of them is ever on screen.
    filtering: bool,
    /// Frame of the spinner shown beside anything the picker is waiting for.
    tick: usize,

    step: Step,
    /// Set while a new branch is being started, including while its destination is chosen.
    naming: Option<Naming>,
    destinations: Vec<Destination>,
    destination_cursor: usize,
    chosen: Option<Chosen>,
    activity: Activity,
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
            filtering: false,
            tick: 0,
            step: if has_repo_step {
                Step::Repo
            } else {
                Step::Branch
            },
            naming: None,
            destinations,
            destination_cursor: 0,
            chosen: None,
            activity: Activity::Choosing,
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

    pub fn step(&self) -> Step {
        self.step
    }

    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    pub fn is_working(&self) -> bool {
        matches!(self.activity, Activity::Working { .. })
    }

    /// The user has chosen; the picker is now a progress display.
    pub fn start_working(&mut self, stage: Stage) {
        self.message = None;
        self.activity = Activity::Working { stage };
    }

    pub fn set_stage(&mut self, stage: Stage) {
        self.activity = Activity::Working { stage };
    }

    /// Advance the spinner one frame. Called by the loop that owns the clock, on a timer
    /// rather than per redraw, so it neither speeds up while the user types nor stalls
    /// while they hold a key down.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn frame(&self) -> usize {
        self.tick
    }

    /// Hold the screen on the step that failed.
    ///
    /// git says its piece over several lines; the prompt is one. Collapsing the whitespace
    /// keeps the newlines out of a widget that would draw them as nothing useful.
    pub fn fail(&mut self, error: String) {
        let error = error.split_whitespace().collect::<Vec<_>>().join(" ");
        let stage = match &self.activity {
            Activity::Working { stage, .. } | Activity::Failed { stage, .. } => stage.clone(),
            Activity::Choosing => Stage::Starting {
                branch: self
                    .chosen
                    .as_ref()
                    .map(|chosen| chosen.name().to_string())
                    .unwrap_or_default(),
            },
        };
        self.activity = Activity::Failed { stage, error };
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

    pub fn is_fetching(&self) -> bool {
        self.data.fetching
    }

    /// Whether the search field has the keyboard, rather than the list.
    pub fn is_filtering(&self) -> bool {
        self.filtering
    }

    /// Say something in the prompt line until the next key. Used for what a background job
    /// has to report — a fetch that could not reach the remote, say.
    pub fn set_message(&mut self, message: String) {
        self.message = Some(message);
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

    /// The branch a new one is being cut from, and the name typed so far. `None` unless a
    /// branch is being started.
    pub fn naming(&self) -> Option<(&str, &str)> {
        self.naming
            .as_ref()
            .map(|naming| (naming.base.name.as_str(), naming.name.as_str()))
    }

    /// What the destination is being chosen for.
    pub fn chosen(&self) -> Option<&Chosen> {
        self.chosen.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::branches::fixtures::*;
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn the_list_is_frozen_while_a_step_is_in_flight() {
        let mut state = fetching();
        let before = state.destination_cursor();
        for code in [KeyCode::Down, KeyCode::Up, KeyCode::Enter, KeyCode::Esc] {
            assert_eq!(
                state.handle_key(key(code)),
                BranchAction::Ignored,
                "{code:?}"
            );
        }
        assert_eq!(state.destination_cursor(), before);
        assert!(state.is_working(), "and it is still working");
    }

    #[test]
    fn ctrl_c_stops_a_fetch_but_not_a_worktree_being_made() {
        let mut state = fetching();
        assert_eq!(ctrl(&mut state, 'c'), BranchAction::Quit);

        // Once herdr has been asked for a worktree, leaving would strand the workspace it
        // made for it. There is no key for that.
        let mut state = fetching();
        state.set_stage(Stage::Creating {
            branch: "chore/deps".into(),
        });
        assert_eq!(ctrl(&mut state, 'c'), BranchAction::Ignored);
    }

    #[test]
    fn the_spinner_runs_on_its_own_rather_than_per_wait() {
        let mut state = fetching();
        state.tick();
        state.tick();
        let before = state.frame();
        assert!(before > 0);

        state.set_stage(Stage::Creating {
            branch: "chore/deps".into(),
        });
        assert_eq!(
            state.frame(),
            before,
            "the spinner carries on from where it was"
        );
        assert!(matches!(
            state.activity(),
            Activity::Working {
                stage: Stage::Creating { .. }
            }
        ));

        state.tick();
        assert_eq!(state.frame(), before + 1);
    }

    #[test]
    fn a_fetch_says_it_is_running_where_the_listing_says_the_same() {
        let mut state = state();
        assert!(state.is_loading(), "the remote listing is still out");
        assert!(!state.is_fetching());

        state.set_data(BranchData {
            loading: false,
            fetching: true,
            ..app_branches()
        });
        assert!(state.is_fetching());
        assert!(!state.is_loading());
    }

    #[test]
    fn a_failure_holds_the_screen_until_it_has_been_read() {
        let mut state = fetching();
        state.fail("could not read from remote repository".into());

        let Activity::Failed { stage, error } = state.activity() else {
            panic!("expected a failure, got {:?}", state.activity());
        };
        assert!(
            matches!(stage, Stage::Fetching { .. }),
            "it carries which step failed: {stage:?}"
        );
        assert!(error.contains("remote repository"));

        assert_eq!(state.handle_key(key(KeyCode::Down)), BranchAction::Ignored);
        assert_eq!(state.handle_key(key(KeyCode::Enter)), BranchAction::Quit);
        assert_eq!(state.handle_key(key(KeyCode::Esc)), BranchAction::Quit);
    }

    #[test]
    fn a_multi_line_failure_is_flattened_into_the_one_line_it_has_to_fit_on() {
        let mut state = fetching();
        state.fail("fatal: could not read from remote\nfatal: could not fetch".into());
        let Activity::Failed { error, .. } = state.activity() else {
            panic!("expected a failure");
        };
        assert_eq!(
            error,
            "fatal: could not read from remote fatal: could not fetch"
        );
    }
}
