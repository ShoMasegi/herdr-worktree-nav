//! What the loop does with a key, an answer that arrives behind the first frame, and a
//! removal that outlives the view that started it.

use super::*;
use crate::app::fakes::{
    fake_gh, fake_git, fake_herdr, until, FakeGh, FakeGit, FakeHerdr, Lost, LostFirst, Recorder,
    Refuses, RefusesFirst, Reports, Started, Unwaited,
};
use crate::app::settled::Settled;
use crate::domain::model::{Branch, CheckoutPath, RepoKey};
use crate::port::{
    AgentStatus, GitRef, RefKind, RefWalk, RemovalOutcome, Slug, Snapshot, Track, Workspace,
    WorkspaceWorktree, Worktree, WorktreeList, WorktreeSource,
};
use crate::ui::panes::PanesState;
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// A git that gives the same answer for every working tree, or the same failure. All
/// three matter: clean is what lets a removal through, and dirty and unreadable are what
/// the two refusals protecting a working agent are made of.
struct Answers(Option<bool>);

impl FakeGit for Answers {
    fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
        self.0
            .ok_or_else(|| anyhow::anyhow!("fatal: not a git repository"))
    }
}
fake_git!(Answers);

impl FakeGh for Answers {}
fake_gh!(Answers);

/// A git that never gets round to naming the repository, so a sweep's `gh` question
/// stays outstanding for exactly as long as the test wants it to. Parking rather than
/// sleeping, so the wait is decided by the test rather than by a duration.
struct Never;

impl FakeGit for Never {
    fn github_slug(&self, _repo_root: &str) -> Result<Option<crate::port::Slug>> {
        std::thread::park();
        unreachable!("nothing unparks this")
    }
}
fake_git!(Never);

impl FakeGh for Never {}
fake_gh!(Never);

/// A `gh` that answers, and remembers what it was asked and how often. Everything the
/// loop does with the sweep's half of the question goes through this.
#[derive(Default)]
struct Answering {
    asked: std::sync::Mutex<Vec<String>>,
    /// What `gh` says. `Err` is a `gh` that refused, which is a sentence for the prompt
    /// line rather than an empty answer — see ADR 0011.
    answer: Option<std::result::Result<crate::port::SettledPullRequests, String>>,
}

impl Answering {
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl FakeGit for Answering {
    fn github_slug(&self, repo_root: &str) -> Result<Option<crate::port::Slug>> {
        self.asked.lock().unwrap().push(repo_root.to_string());
        Ok(crate::port::Slug::owner_repo("me", "app"))
    }

    fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
        Ok(false)
    }
}
fake_git!(Answering);

impl FakeGh for Answering {
    fn settled_pull_requests(
        &self,
        _slug: &crate::port::Slug,
    ) -> std::result::Result<crate::port::SettledPullRequests, String> {
        self.answer
            .clone()
            .unwrap_or_else(|| Ok(crate::port::SettledPullRequests::All(Vec::new())))
    }
}
fake_gh!(Answering);

/// A `gh` that refuses the first time it is asked and answers every time after: the
/// network coming back, or a token renewed.
#[derive(Default)]
struct Recovering {
    asked: std::sync::Mutex<usize>,
}

impl Recovering {
    fn asked(&self) -> usize {
        *self.asked.lock().unwrap()
    }
}

impl FakeGit for Recovering {
    fn github_slug(&self, _repo_root: &str) -> Result<Option<crate::port::Slug>> {
        Ok(crate::port::Slug::owner_repo("me", "app"))
    }

    fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
        Ok(false)
    }
}
fake_git!(Recovering);

impl FakeGh for Recovering {
    fn settled_pull_requests(
        &self,
        _slug: &crate::port::Slug,
    ) -> std::result::Result<crate::port::SettledPullRequests, String> {
        let mut asked = self.asked.lock().unwrap();
        *asked += 1;
        if *asked == 1 {
            return Err("gh refused the question this asked: could not connect".into());
        }
        Ok(crate::port::SettledPullRequests::All(Vec::new()))
    }
}
fake_gh!(Recovering);

fn press(state: &mut PanesState, key: char) {
    state.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char(key),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

const FEAT_LOGIN: &str = "/wt/feat-login";
const FIX_CRASH: &str = "/wt/fix-crash";

/// A herdr that can describe its session and a git that answers for it — everything
/// `collect_tree` reads. One repository: its own checkout with a pane in it, and two
/// finished worktrees with none. The second reading can differ from the first in one of
/// the two ways issue #25 is about.
struct Session {
    /// A pane opened in `feat/login` between the first reading and the second.
    pane_opens: bool,
    /// A file written into `feat/login` between the first walk and the second.
    file_written: bool,
    /// A herdr that no longer has what the rows say it has: the state a list the picker
    /// has not caught up with puts every key into.
    stale_rows: bool,
    readings: Mutex<usize>,
    walks: Mutex<BTreeMap<String, usize>>,
}

impl Session {
    fn new(pane_opens: bool, file_written: bool) -> Self {
        Self {
            pane_opens,
            file_written,
            stale_rows: false,
            readings: Mutex::new(0),
            walks: Mutex::new(BTreeMap::new()),
        }
    }

    /// A session that still describes itself but refuses to act on any of it.
    fn with_stale_rows() -> Self {
        Self {
            stale_rows: true,
            ..Self::new(false, false)
        }
    }

    fn pane_is_open(&self) -> bool {
        self.pane_opens && *self.readings.lock().unwrap() >= 2
    }

    /// How often the walk asked about this checkout.
    fn walks_of(&self, checkout_path: &str) -> usize {
        self.walks
            .lock()
            .unwrap()
            .get(checkout_path)
            .copied()
            .unwrap_or(0)
    }
}

fn snapshot_pane(id: &str, cwd: &str) -> crate::port::Pane {
    let workspace = id.split(':').next().unwrap().to_string();
    crate::port::Pane {
        pane_id: id.into(),
        tab_id: format!("{workspace}:t1"),
        workspace_id: workspace,
        terminal_id: String::new(),
        cwd: Some(cwd.into()),
        foreground_cwd: None,
        focused: false,
        agent: Some("codex".into()),
        agent_status: AgentStatus::Idle,
        title: None,
        terminal_title_stripped: None,
        label: None,
    }
}

fn workspace(id: &str, checkout_path: &str, linked: bool) -> Workspace {
    Workspace {
        workspace_id: id.into(),
        label: String::new(),
        number: 0,
        focused: false,
        active_tab_id: None,
        agent_status: AgentStatus::Unknown,
        worktree: Some(WorkspaceWorktree {
            repo_key: "/src/app/.git".into(),
            repo_name: "app".into(),
            repo_root: "/src/app".into(),
            checkout_path: checkout_path.into(),
            is_linked_worktree: linked,
        }),
    }
}

fn listed(branch: &str, path: &str, linked: bool, open_in: Option<&str>) -> Worktree {
    Worktree {
        branch: Some(branch.into()),
        path: path.into(),
        label: String::new(),
        is_bare: false,
        is_detached: false,
        is_linked_worktree: linked,
        is_prunable: false,
        open_workspace_id: open_in.map(str::to_string),
    }
}

fn gone(branch: &str, path: &str) -> GitRef {
    GitRef {
        name: branch.into(),
        kind: RefKind::Local,
        committed_at: None,
        subject: None,
        upstream: Some(format!("origin/{branch}")),
        track: Some(Track::Gone),
        worktree_path: Some(path.into()),
    }
}

impl FakeHerdr for Session {
    fn snapshot(&self) -> Result<Snapshot> {
        *self.readings.lock().unwrap() += 1;
        let mut workspaces = vec![workspace("w1", "/src/app", false)];
        let mut panes = vec![snapshot_pane("w1:p1", "/src/app")];
        if self.pane_is_open() {
            workspaces.push(workspace("w2", FEAT_LOGIN, true));
            panes.push(snapshot_pane("w2:p1", FEAT_LOGIN));
        }
        Ok(Snapshot {
            workspaces,
            panes,
            ..Snapshot::default()
        })
    }

    fn worktree_list(&self, _cwd: &str) -> Result<WorktreeList> {
        Ok(WorktreeList {
            source: WorktreeSource {
                repo_key: "/src/app/.git".into(),
                repo_name: "app".into(),
                repo_root: "/src/app".into(),
                source_checkout_path: "/src/app".into(),
                source_workspace_id: None,
            },
            worktrees: vec![
                listed("main", "/src/app", false, Some("w1")),
                listed(
                    "feat/login",
                    FEAT_LOGIN,
                    true,
                    self.pane_is_open().then_some("w2"),
                ),
                listed("fix/crash", FIX_CRASH, true, None),
            ],
        })
    }

    fn worktree_open(&self, _req: &WorktreeOpen) -> Result<crate::port::WorktreeOpened> {
        assert!(self.stale_rows, "only a stale row opens a checkout here");
        Err(anyhow::anyhow!(
            "herdr rejected worktree.open: no such path (not_found)"
        ))
    }

    fn pane_focus(&self, _pane_id: &str) -> Result<()> {
        match self.stale_rows {
            true => Err(anyhow::anyhow!(
                "herdr rejected pane.focus: no such pane (not_found)"
            )),
            false => Ok(()),
        }
    }
}
fake_herdr!(Session);

impl FakeGit for Session {
    fn is_dirty(&self, checkout_path: &str) -> Result<bool> {
        let mut walks = self.walks.lock().unwrap();
        let walked = walks.entry(checkout_path.to_string()).or_insert(0);
        *walked += 1;
        Ok(self.file_written && checkout_path == FEAT_LOGIN && *walked >= 2)
    }

    fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
        Ok(Slug::owner_repo("me", "app"))
    }

    fn local_refs(&self, _repo_root: &str) -> Result<RefWalk> {
        Ok(RefWalk::of(vec![
            gone("feat/login", FEAT_LOGIN),
            gone("fix/crash", FIX_CRASH),
        ]))
    }
}
fake_git!(Session);

impl FakeGh for Session {
    fn settled_pull_requests(
        &self,
        _slug: &Slug,
    ) -> std::result::Result<crate::port::SettledPullRequests, String> {
        Ok(crate::port::SettledPullRequests::All(Vec::new()))
    }
}
fake_gh!(Session);

/// The two panes [`Closing`] starts with, in the order a removal names them.
const FIRST_PANE: &str = "w2:p1";
const SECOND_PANE: &str = "w2:p2";

/// A herdr that really closes panes and then describes a session without them, and a git
/// that answers for it. One repository: its own checkout, and `feat/login` with two panes
/// in it.
///
/// `Recorder` cannot reach this ground: it refuses to describe itself at all, and what
/// the list says once some of the panes have gone needs a herdr that answers.
struct Closing {
    /// The one pane it will not close. Everything the removal names ahead of that does
    /// close, which is what makes a close that stopped partway reachable at all.
    refuses: &'static str,
    open: Mutex<Vec<String>>,
    /// Whether the checkout goes when the last of its panes does — the ordinary ending,
    /// where the removal went through between one reading and the next.
    removed_with_its_panes: bool,
}

impl Closing {
    fn refusing(pane_id: &'static str) -> Self {
        Self {
            refuses: pane_id,
            open: Mutex::new(vec![FIRST_PANE.to_string(), SECOND_PANE.to_string()]),
            removed_with_its_panes: false,
        }
    }

    /// Closes whatever it is asked to, and describes a session the checkout has left.
    fn where_the_checkout_goes_too() -> Self {
        Self {
            refuses: "",
            removed_with_its_panes: true,
            ..Self::refusing("")
        }
    }

    /// Whether `feat/login` is still there, which is what the two readings differ by.
    fn listed(&self) -> bool {
        !self.removed_with_its_panes || !self.open.lock().unwrap().is_empty()
    }
}

impl FakeHerdr for Closing {
    fn pane_close(&self, pane_id: &str) -> Result<()> {
        if pane_id == self.refuses {
            return Err(anyhow::anyhow!(
                "herdr rejected pane.close: no such pane (not_found)"
            ));
        }
        self.open.lock().unwrap().retain(|open| open != pane_id);
        Ok(())
    }

    fn snapshot(&self) -> Result<Snapshot> {
        let mut workspaces = vec![workspace("w1", "/src/app", false)];
        // The repository is only in the tree at all while a pane of it is: `collect`
        // probes from pane placements. The checkout that goes here takes the last of its
        // own panes with it, so this keeps one in the main checkout.
        let mut panes = match self.removed_with_its_panes {
            true => vec![snapshot_pane("w1:p9", "/src/app")],
            false => Vec::new(),
        };
        if self.listed() {
            workspaces.push(workspace("w2", FEAT_LOGIN, true));
        }
        panes.extend(
            self.open
                .lock()
                .unwrap()
                .iter()
                .map(|pane_id| snapshot_pane(pane_id, FEAT_LOGIN)),
        );
        Ok(Snapshot {
            workspaces,
            panes,
            ..Snapshot::default()
        })
    }

    fn worktree_list(&self, _cwd: &str) -> Result<WorktreeList> {
        let mut worktrees = vec![listed("main", "/src/app", false, Some("w1"))];
        if self.listed() {
            worktrees.push(listed("feat/login", FEAT_LOGIN, true, Some("w2")));
        }
        Ok(WorktreeList {
            source: WorktreeSource {
                repo_key: "/src/app/.git".into(),
                repo_name: "app".into(),
                repo_root: "/src/app".into(),
                source_checkout_path: "/src/app".into(),
                source_workspace_id: None,
            },
            worktrees,
        })
    }
}
fake_herdr!(Closing);

impl FakeGit for Closing {
    fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
        Ok(false)
    }

    fn local_refs(&self, _repo_root: &str) -> Result<RefWalk> {
        Ok(RefWalk::of(vec![gone("feat/login", FEAT_LOGIN)]))
    }

    fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
        Ok(Slug::owner_repo("me", "app"))
    }
}
fake_git!(Closing);

/// A herdr whose refusal has a cause under it, which is the shape the socket adapter
/// makes: `with_context` over the io error that actually happened. Every other fake here
/// refuses with one flat sentence.
struct Chained;

impl FakeHerdr for Chained {
    fn pane_close(&self, _pane_id: &str) -> Result<()> {
        Ok(())
    }

    fn snapshot(&self) -> Result<Snapshot> {
        Err(anyhow::anyhow!("broken pipe").context("sending session.snapshot to herdr"))
    }
}
fake_herdr!(Chained);

/// Open the picker on `session` the way `run` does, enter a sweep with both finished
/// worktrees marked, and press `Enter`.
fn enter_pressed(session: &Arc<Session>) -> (PanesState, Pending) {
    let (_, tree) = collect::collect_tree(&**session, &**session).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut pending = Pending {
        dirty: Dirty::new(session.clone()),
        settled: Settled::new(session.clone(), session.clone()),
    };
    pending.dirty.ask(state.tree());
    press(&mut state, 'S');
    settle(&mut state, &mut pending);
    let chosen: Vec<String> = state
        .chosen()
        .into_iter()
        .map(|(_, path)| path.as_str().to_string())
        .collect();
    assert_eq!(
        chosen,
        [FEAT_LOGIN, FIX_CRASH],
        "both are gone, clean, and have nothing running in them"
    );
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::SweepReload);
    (state, pending)
}

/// `enter_pressed`, then the re-read it asked for and the walk that starts, driven to
/// the frame that puts the box up.
fn sweep_asked(session: &Arc<Session>) -> (PanesState, Pending) {
    let (mut state, mut pending) = enter_pressed(session);
    re_read(
        &mut state,
        &mut pending,
        &**session,
        &**session,
        ReRead::ForSweep,
    );
    settle(&mut state, &mut pending);
    (state, pending)
}

/// What the sweep's box lists, by path.
fn box_paths(state: &PanesState) -> Vec<String> {
    state
        .pending_sweep()
        .map(|sweep| {
            sweep
                .removals()
                .iter()
                .map(|removal| removal.checkout_path().as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_pane_opened_after_the_sweep_began_takes_that_checkout_off_the_list_before_anything_is_removed()
{
    let session = Arc::new(Session::new(true, false));
    let (mut state, _pending) = sweep_asked(&session);
    assert_eq!(
        box_paths(&state),
        [FIX_CRASH],
        "the checkout with the pane is off the list"
    );
    assert_eq!(state.message(), Some("no longer marked: feat/login"));

    // And what `y` removes is what the box listed.
    let recorder = Recorder::default();
    let port = Started(&recorder);
    let mut removals = Removals::new(&port);
    let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
        panic!("`y` is the answer that goes ahead");
    };
    start_sweep(&mut state, &mut removals, &recorder, &sweep);
    assert_eq!(
        recorder.did(),
        ["start /wt/fix-crash after 0 deleting the branch"]
    );
}

#[test]
fn a_file_written_after_the_sweep_began_does_the_same() {
    let session = Arc::new(Session::new(false, true));
    let (state, _pending) = sweep_asked(&session);
    assert_eq!(
        session.walks_of(FEAT_LOGIN),
        2,
        "the working tree was walked again"
    );
    assert_eq!(box_paths(&state), [FIX_CRASH]);
    assert_eq!(state.message(), Some("no longer marked: feat/login"));
}

#[test]
fn one_refusal_to_start_does_not_stop_the_rest_of_the_sweep() {
    let session = Arc::new(Session::new(false, false));
    let (mut state, _pending) = sweep_asked(&session);
    assert_eq!(box_paths(&state), [FEAT_LOGIN, FIX_CRASH]);

    let recorder = Recorder::default();
    let port = RefusesFirst::new(&recorder);
    let mut removals = Removals::new(&port);
    let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
        panic!("`y` is the answer that goes ahead");
    };
    start_sweep(&mut state, &mut removals, &recorder, &sweep);

    assert_eq!(
        recorder.did(),
        ["start /wt/fix-crash after 0 deleting the branch"],
        "the second started although the first would not"
    );
    assert_eq!(
        state.message(),
        Some(
            "could not start removing feat/login: could not spawn: no such file or \
             directory"
        )
    );
    assert_eq!(removals.paths(), [CheckoutPath::for_test(FIX_CRASH)]);
    assert!(
        state.rows().iter().any(|row| row.is_removing),
        "and the row that is going says so"
    );
}

#[test]
fn a_re_read_that_fails_asks_nothing_over_the_old_facts() {
    // `Enter` asks for the re-read so that the box is about the disk as it is; with
    // herdr not answering, a box would be about the disk as it was.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = enter_pressed(&session);
    re_read(
        &mut state,
        &mut pending,
        &Recorder::default(),
        &*session,
        ReRead::ForSweep,
    );
    settle(&mut state, &mut pending);

    assert!(state.pending_sweep().is_none());
    assert_eq!(state.message(), Some("nothing was asked"));
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
    assert_eq!(
        state.handle_key(key(KeyCode::Char('y'))),
        Action::Ignored,
        "and `y` answers nothing"
    );
    assert_eq!(
        state.handle_key(key(KeyCode::Enter)),
        Action::SweepReload,
        "asking again asks for the re-read again"
    );
}

#[test]
fn enter_keeps_what_gh_said_and_r_does_not() {
    // `gh` only widens the sweep, so `Enter`'s re-read keeps its answers — ADR 0011.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = enter_pressed(&session);
    assert_eq!(
        pending.settled.answers(state.tree()).len(),
        1,
        "the sweep asked gh"
    );

    re_read(
        &mut state,
        &mut pending,
        &*session,
        &*session,
        ReRead::ForSweep,
    );
    assert_eq!(
        pending.settled.answers(state.tree()).len(),
        1,
        "and `Enter` kept it"
    );

    re_read(&mut state, &mut pending, &*session, &*session, ReRead::Key);
    assert!(
        pending.settled.answers(state.tree()).is_empty(),
        "`r` forgets it"
    );
}

/// Drive the loop's per-frame step until nothing is outstanding, the way `run` does.
fn settle(state: &mut PanesState, pending: &mut Pending) {
    until("the frame never stopped waiting", || {
        !show_answers(state, pending)
    });
}

/// A walk in progress and a sweep whose `gh` question will not come back.
fn pending_on_gh(dirty: Dirty) -> Pending {
    let port = std::sync::Arc::new(Never);
    Pending {
        dirty,
        settled: crate::app::settled::Settled::new(port.clone(), port),
    }
}

/// A walk in progress and a sweep nobody has entered. `show_answers` asks `gh` nothing
/// until one is, which is what lets these ports say `unreachable!()` and mean it.
fn pending(dirty: Dirty) -> Pending {
    let port = std::sync::Arc::new(Answers(Some(false)));
    Pending {
        dirty,
        settled: crate::app::settled::Settled::new(port.clone(), port),
    }
}

fn no_pane_tree() -> crate::domain::model::Tree {
    let mut tree = one_pane_tree();
    tree.repos[0].worktrees[0].panes.clear();
    tree
}

fn one_pane_tree() -> crate::domain::model::Tree {
    use crate::domain::model::{PaneNode, Refs, RepoNode, Tree, WorktreeNode};
    Tree {
        repos: vec![RepoNode {
            repo_key: "/src/app/.git".into(),
            repo_root: "/src/app".into(),
            display_name: "me/app".into(),
            refs: Refs::Read,
            worktrees: vec![WorktreeNode {
                branch: Branch::Out("feat/login".into()),
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

/// A picker open on `session`, the way `run` starts one.
fn opened(session: &Arc<Session>) -> (PanesState, Pending) {
    let (_, tree) = collect::collect_tree(&**session, &**session).expect("the first reading");
    let state = PanesState::new(tree, None);
    let mut pending = Pending {
        dirty: Dirty::new(session.clone()),
        settled: Settled::new(session.clone(), session.clone()),
    };
    pending.dirty.ask(state.tree());
    (state, pending)
}

#[test]
fn a_pane_herdr_no_longer_has_leaves_the_picker_up_and_says_what_herdr_said() {
    // The account of it was `herdr plugin log list`, which is not where the user is:
    // from their seat the picker vanished the way it does when a jump worked. Issue #65.
    let session = Arc::new(Session::with_stale_rows());
    let (mut state, mut pending) = opened(&session);
    let before = *session.readings.lock().unwrap();

    let exit = act(
        &mut state,
        &mut pending,
        &*session,
        &*session,
        Action::Jump("w1:p1".into()),
    );

    assert!(exit.is_none(), "the picker stays up");
    let said = state.message().expect("the prompt line says what happened");
    assert!(said.starts_with("could not go to that pane"), "{said}");
    assert!(said.contains("not_found"), "herdr's own words: {said}");
    assert!(
        *session.readings.lock().unwrap() > before,
        "and the list that was behind is read again"
    );
    assert!(
        state.trouble().is_none(),
        "the reading worked, so nothing is"
    );
}

#[test]
fn a_checkout_that_is_no_longer_there_does_the_same() {
    let session = Arc::new(Session::with_stale_rows());
    let (mut state, mut pending) = opened(&session);

    let exit = act(
        &mut state,
        &mut pending,
        &*session,
        &*session,
        Action::OpenWorktree {
            repo_root: "/src/app".into(),
            checkout_path: FIX_CRASH.into(),
        },
    );

    assert!(exit.is_none(), "the picker stays up");
    let said = state.message().expect("the prompt line says what happened");
    assert!(said.starts_with("could not open that checkout"), "{said}");
    assert!(said.contains("not_found"), "herdr's own words: {said}");
}

#[test]
fn a_jump_that_worked_still_closes_the_picker() {
    // The other half of the rule: leaving is the right answer for a key that worked,
    // and a refusal is the only thing that holds the picker open.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = opened(&session);

    let exit = act(
        &mut state,
        &mut pending,
        &*session,
        &*session,
        Action::Jump("w1:p1".into()),
    );

    assert!(
        matches!(exit, Some(Exit::Closed)),
        "a jump that worked closes the picker"
    );
    assert_eq!(state.message(), None);
}

#[test]
fn the_branches_exit_carries_the_runtime_no_pane_toggle() {
    let mut state = PanesState::new(no_pane_tree(), None);
    state.handle_key(key(KeyCode::Char('p')));
    let action = state.handle_key(key(KeyCode::Tab));

    match finish(&Recorder::default(), action, &state).unwrap() {
        Exit::ShowBranches {
            show_worktrees_without_panes,
            ..
        } => assert!(!show_worktrees_without_panes),
        Exit::Closed => panic!("Tab must switch to the branches view"),
    }
}

#[test]
fn the_loop_hands_on_a_dirty_answer_too_or_nothing_is_ever_protected() {
    // A clean answer alone is what clears "still reading"; the dirty answer is what the
    // refusal is made of. Hand on the first without the second and `Shift-D` walks into
    // the confirmation box for a checkout full of working agents, and `y` closes every
    // one of their panes before git refuses to remove it.
    let mut state = PanesState::new(one_pane_tree(), None);
    let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(true))));
    dirty.ask(state.tree());

    let mut pending = pending(dirty);
    until(
        "the walk never answered for the only checkout there is",
        || !show_answers(&mut state, &mut pending),
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

/// The same, where the tree has more than one checkout in it and the panes are in the
/// second: what [`Closing`] describes.
fn removal_of(state: &PanesState, checkout_path: &str) -> Removal {
    let repo = &state.tree().repos[0];
    let worktree = repo
        .worktrees
        .iter()
        .find(|worktree| worktree.checkout_path == checkout_path)
        .expect("the tree has that checkout");
    Removal::of(&repo.repo_root, worktree)
}

/// The panes the list is drawing, in order. A pane that has been closed and is still on
/// screen is a row the cursor stops on and `Enter` acts on.
fn pane_rows(state: &PanesState) -> Vec<String> {
    state
        .rows()
        .iter()
        .filter(|row| matches!(row.reference, crate::domain::rows::RowRef::Pane(..)))
        .map(|row| row.meta.clone())
        .collect()
}

/// The one-pane tree with a second pane in the same checkout, so that a close can stop
/// between them.
fn two_pane_tree() -> crate::domain::model::Tree {
    let mut tree = one_pane_tree();
    let panes = &mut tree.repos[0].worktrees[0].panes;
    let mut second = panes[0].clone();
    second.pane_id = SECOND_PANE.into();
    panes.push(second);
    tree
}

#[test]
fn gh_is_asked_when_a_sweep_is_entered_and_not_before() {
    // The heavier of the two `gh` calls, so ADR 0011 defers it: most sessions never
    // sweep, and one that does should not pay for it on every picker open.
    let port = std::sync::Arc::new(Answering::default());
    let mut state = PanesState::new(no_pane_tree(), None);
    let mut pending = Pending {
        dirty: Dirty::new(port.clone()),
        settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
    };
    pending.dirty.ask(state.tree());
    settle(&mut state, &mut pending);
    assert!(
        port.asked().is_empty(),
        "the picker opened and nobody swept"
    );

    press(&mut state, 'S');
    settle(&mut state, &mut pending);
    assert_eq!(
        port.asked(),
        ["/src/app"],
        "asked once, about the repository root"
    );

    // And a second frame, and leaving and coming back, ask nothing more: a merged pull
    // request does not become unmerged.
    settle(&mut state, &mut pending);
    press(&mut state, 'q');
    press(&mut state, 'S');
    settle(&mut state, &mut pending);
    assert_eq!(port.asked().len(), 1);
}

#[test]
fn what_gh_said_reaches_the_rows_and_what_went_wrong_reaches_the_prompt() {
    let port = std::sync::Arc::new(Answering {
        answer: Some(Err(
            "gh refused the question this asked: no auth".to_string()
        )),
        ..Answering::default()
    });
    let mut state = PanesState::new(no_pane_tree(), None);
    let mut pending = Pending {
        dirty: Dirty::new(port.clone()),
        settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
    };
    pending.dirty.ask(state.tree());

    press(&mut state, 'S');
    settle(&mut state, &mut pending);

    assert_eq!(
        state.sweep_trouble(),
        Some("me/app: gh refused the question this asked: no auth"),
        "the rows can say a checkout could not be judged; only this says which and why"
    );
    assert!(!state.is_asking_gh(), "and the spinner has stopped");
}

#[test]
fn a_pull_request_gh_found_widens_the_sweep_through_the_loop() {
    let port = std::sync::Arc::new(Answering {
        answer: Some(Ok(crate::port::SettledPullRequests::All(vec![
            crate::port::SettledPullRequest {
                number: 9,
                head_ref: "feat/login".to_string(),
                from_a_fork: false,
                outcome: crate::port::PullRequestOutcome::Merged,
            },
        ]))),
        ..Answering::default()
    });
    let mut state = PanesState::new(no_pane_tree(), None);
    let mut pending = Pending {
        dirty: Dirty::new(port.clone()),
        settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
    };
    pending.dirty.ask(state.tree());

    press(&mut state, 'S');
    settle(&mut state, &mut pending);

    assert_eq!(
        state.chosen(),
        vec![(
            RepoKey::of(&state.tree().repos[0]),
            CheckoutPath::for_test("/wt/feat-login")
        )],
        "git had nothing to say about it; gh may only widen, and this is widening"
    );
}

#[test]
fn a_reload_asks_gh_again_because_a_pull_request_can_land_while_the_picker_is_up() {
    let port = std::sync::Arc::new(Answering::default());
    let mut state = PanesState::new(no_pane_tree(), None);
    let mut pending = Pending {
        dirty: Dirty::new(port.clone()),
        settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
    };
    pending.dirty.ask(state.tree());
    press(&mut state, 'S');
    settle(&mut state, &mut pending);
    assert_eq!(port.asked().len(), 1);

    // What the `r` arm does to the sweep's half. `r` itself is `Ignored` during a sweep,
    // so this is the state the arm leaves behind rather than the key.
    pending.settled.forget();
    settle(&mut state, &mut pending);
    assert_eq!(
        port.asked().len(),
        2,
        "the reload asks again rather than showing what it had"
    );
}

#[test]
fn entering_a_sweep_again_asks_gh_again_where_it_refused() {
    // Without this, a network out for one keypress is out for the life of the picker.
    let port = std::sync::Arc::new(Recovering::default());
    let mut state = PanesState::new(no_pane_tree(), None);
    let mut pending = Pending {
        dirty: Dirty::new(port.clone()),
        settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
    };
    pending.dirty.ask(state.tree());

    press(&mut state, 'S');
    settle(&mut state, &mut pending);
    assert_eq!(
        state.sweep_trouble(),
        Some("me/app: gh refused the question this asked: could not connect")
    );

    press(&mut state, 'S');
    press(&mut state, 'S');
    settle(&mut state, &mut pending);
    assert_eq!(
        state.sweep_trouble(),
        None,
        "asked again on the way back in, and this time it answered"
    );
    assert_eq!(port.asked(), 2, "and not on any frame between");
}

#[test]
fn the_loop_keeps_its_clock_while_the_sweep_is_waiting_on_gh() {
    // With `gh` left out of what `show_answers` answers, `asking gh…` draws frame zero
    // for ever and the answer reaches the rows only when the user presses a key.
    let mut state = PanesState::new(no_pane_tree(), None);
    let mut pending = pending_on_gh(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));

    // The walk has nothing outstanding; the sweep does.
    state.handle_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('S'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert!(state.is_sweeping());
    pending.settled.ask(state.tree());

    assert!(
        show_answers(&mut state, &mut pending),
        "gh is still out, so the loop still has something to wake up for"
    );
    assert!(state.is_asking_gh(), "and the prompt line says which");
}

#[test]
fn a_yes_starts_the_removal_and_says_on_the_row_that_it_is_going() {
    // The arm behind `y`. Deleted, the confirmation box closes and nothing happens: no
    // error, no message, the row unchanged.
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
    assert_eq!(removals.paths(), [CheckoutPath::for_test("/wt/feat-login")]);
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
fn panes_that_have_certainly_closed_are_not_left_on_screen_without_a_word() {
    // The branch the other two never enter: both use a checkout with no panes. Here the
    // panes did close, so saying nothing leaves rows for panes that have stopped, with
    // the cursor able to jump to them.
    let recorder = Recorder::default();
    let port = Started(&recorder);
    let mut removals = Removals::new(&port);
    let mut state = PanesState::new(one_pane_tree(), None);
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
        None,
        "the removal started; it is the list that could not be caught up"
    );
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
    assert!(
        state.rows().iter().any(|row| row.is_removing),
        "and the removal is still shown as going, because it is"
    );
}

#[test]
fn a_close_that_stopped_partway_takes_the_panes_it_closed_off_the_list() {
    // One pane is gone and the other is not, so the list is known wrong rather than
    // merely stale. Left drawn, `Enter` on the row for the one that is gone fails
    // `pane_focus` and carries that out of `run` through `perform` — the picker
    // disappears, with the reason only in the log.
    let herdr = Arc::new(Closing::refusing(SECOND_PANE));
    let mut removals = Removals::new(&Refuses);
    let (_, tree) = collect::collect_tree(&*herdr, &*herdr).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut dirty = Dirty::new(herdr.clone());
    let removal = removal_of(&state, FEAT_LOGIN);
    assert_eq!(pane_rows(&state), [FIRST_PANE, SECOND_PANE]);

    start_removal(
        &mut state,
        &mut dirty,
        &mut removals,
        &*herdr,
        &*herdr,
        &removal,
    );

    assert_eq!(
        state.message(),
        Some(
            "could not close w2:p2: herdr rejected pane.close: no such pane (not_found) \
             — 1 of its 2 panes was closed first, and the checkout was not removed"
        ),
        "the account of what happened is kept whole"
    );
    assert_eq!(
        pane_rows(&state),
        [SECOND_PANE],
        "and the pane that did close is off the list without anyone pressing `r`"
    );
}

#[test]
fn a_close_that_stopped_partway_says_so_when_the_list_will_not_be_read_either() {
    // The order is what is pinned here: the panes that stopped are what nothing else on
    // screen will mention, so the list is appended rather than written over them.
    let recorder = Recorder::refusing(SECOND_PANE);
    let git = Answers(Some(false));
    let mut removals = Removals::new(&Refuses);
    let mut state = PanesState::new(two_pane_tree(), None);
    let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
    let removal = only_removal(&state);

    start_removal(
        &mut state,
        &mut dirty,
        &mut removals,
        &recorder,
        &git,
        &removal,
    );

    assert_eq!(state.message(), Some("could not close w2:p2: herdr rejected pane.close: no such pane (not_found) — 1 of its 2 panes was closed first, and the checkout was not removed"));
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
}

#[test]
fn a_close_that_refused_at_the_first_pane_reads_the_list_anyway() {
    // A count of none is not the same as nothing having happened: `pane_close` answers
    // `Ok` for a pane that had already gone, so an `Err` is a call this side cannot
    // account for. `Recorder` will not describe itself, so the reading asked for here
    // arrives as a clause on the end of this line.
    let recorder = Recorder::refusing(FIRST_PANE);
    let git = Answers(Some(false));
    let mut removals = Removals::new(&Refuses);
    let mut state = PanesState::new(two_pane_tree(), None);
    let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
    let removal = only_removal(&state);

    start_removal(
        &mut state,
        &mut dirty,
        &mut removals,
        &recorder,
        &git,
        &removal,
    );

    assert_eq!(state.message(), Some("could not close w2:p1: herdr rejected pane.close: no such pane (not_found) — none of its 2 panes were closed, and the checkout was not removed"));
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
}

#[test]
fn a_working_tree_git_would_not_read_is_handed_on_as_its_own_refusal() {
    // Folded into dirty or into clean, a checkout git could not answer for is offered
    // for removal on the strength of an answer nobody gave.
    let mut state = PanesState::new(one_pane_tree(), None);
    let mut dirty = Dirty::new(std::sync::Arc::new(Answers(None)));
    dirty.ask(state.tree());

    let mut pending = pending(dirty);
    until(
        "the walk never answered for the only checkout there is",
        || !show_answers(&mut state, &mut pending),
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
    // With the answers never handed on, every checkout with panes says "still reading
    // that working tree" for the life of the picker and the feature is unreachable.
    let mut state = PanesState::new(one_pane_tree(), None);
    let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
    dirty.ask(state.tree());

    let mut pending = pending(dirty);
    until(
        "the walk never answered for the only checkout there is",
        || !show_answers(&mut state, &mut pending),
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

#[test]
fn every_refusal_to_start_is_on_the_prompt_line() {
    // Two children that never ran have no toast to speak for them, so the line has to
    // carry both — one overwriting the other would lose a checkout the box listed.
    let session = Arc::new(Session::new(false, false));
    let (mut state, _pending) = sweep_asked(&session);
    let recorder = Recorder::default();
    let mut removals = Removals::new(&Refuses);
    let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
        panic!("`y` is the answer that goes ahead");
    };
    start_sweep(&mut state, &mut removals, &recorder, &sweep);

    assert_eq!(
        state.message(),
        Some(
            "could not start removing feat/login: could not spawn: no such file or \
             directory; could not start removing fix/crash: could not spawn: no such \
             file or directory"
        )
    );
    assert!(removals.is_empty(), "nothing started");
    assert!(!state.is_sweeping(), "and the sweep is over either way");
}

#[test]
fn a_reload_that_fails_leaves_the_list_saying_it_is_behind() {
    // `r` asks no question, so its failure has nothing to withdraw: there is no account
    // of this moment to give, only the list still being what it was. That is the same
    // condition a removal's failed reading leaves, reached the other way, and it says so
    // in the same words rather than in herdr's alone.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = enter_pressed(&session);
    re_read(
        &mut state,
        &mut pending,
        &Recorder::default(),
        &*session,
        ReRead::Key,
    );
    assert_eq!(state.message(), None);
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
}

#[test]
fn two_removals_reporting_in_one_frame_share_the_line() {
    // A sweep starts them together, so they can come home together.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = sweep_asked(&session);
    let port = Reports(RemovalOutcome::BranchKept("error: not fully merged".into()));
    let mut removals = Removals::new(&port);
    let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
        panic!("`y` is the answer that goes ahead");
    };
    start_sweep(&mut state, &mut removals, &Recorder::default(), &sweep);
    // Both answers in the channel before the one drain. Draining until it empties is not
    // the same test: the two can arrive in separate frames, and the second report then
    // replaces the first on the line.
    until("both reported", || removals.reported() == 2);
    drain_finished(
        &mut state,
        &mut pending,
        &mut removals,
        &*session,
        &*session,
    );

    let line = state.message().expect("two kept branches are said");
    for said in [
        "removed feat/login, branch kept: error: not fully merged",
        "removed fix/crash, branch kept: error: not fully merged",
    ] {
        assert!(line.contains(said), "{said:?} missing from {line:?}");
    }
    assert_eq!(
        line.matches("; ").count(),
        1,
        "one line, two reports: {line:?}"
    );
}

#[test]
fn a_question_the_re_read_took_back_is_said_last() {
    // The withdrawal is the one thing on the line with no toast behind it.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = sweep_asked(&session);
    let port = Reports(RemovalOutcome::BranchKept("error: not fully merged".into()));
    let mut removals = Removals::new(&port);
    let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
    removals.remove(&Recorder::default(), &earlier).unwrap();
    until("it reported", || {
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );
        removals.is_empty()
    });

    assert!(
        state.pending_sweep().is_none(),
        "the box went with the re-read"
    );
    assert_eq!(
        state.message(),
        Some(
            "removed feat/login, branch kept: error: not fully merged; the list changed \
             while that was up — ask again"
        )
    );
}

#[test]
fn a_question_taken_back_after_two_reports_is_still_said_last() {
    // Two reports and one reading, which is the frame's: the notice belongs to the frame
    // rather than to the report that happened to come last in it.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = sweep_asked(&session);
    let port = Reports(RemovalOutcome::BranchKept("error: not fully merged".into()));
    let mut removals = Removals::new(&port);
    let earlier: Vec<Removal> = state.tree().repos[0].worktrees[1..=2]
        .iter()
        .map(|worktree| Removal::sweeping("/src/app", worktree))
        .collect();
    for removal in &earlier {
        removals.remove(&Recorder::default(), removal).unwrap();
    }
    // Both answers in the channel before the one drain: reading one to find out would
    // take it, so the count is what a test can wait on.
    until("both reported", || removals.reported() == 2);
    drain_finished(
        &mut state,
        &mut pending,
        &mut removals,
        &*session,
        &*session,
    );

    assert!(removals.is_empty(), "both came home in the one drain");
    let line = state
        .message()
        .expect("two reports and a withdrawal are said");
    assert!(
        line.ends_with(WITHDRAWN),
        "the withdrawal comes last: {line:?}"
    );
    assert_eq!(
        line.matches("; ").count(),
        2,
        "three things on one line: {line:?}"
    );
}

#[test]
fn a_removal_that_ended_without_saying_what_happened_reads_the_list_again_and_says_so() {
    // Left out, the row simply stops spinning and reads as a checkout that is still
    // standing, which is the one thing nobody knows. The session's second reading differs
    // from its first, so the list having been read is something this can see.
    let session = Arc::new(Session::new(true, false));
    let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut pending = pending(Dirty::new(session.clone()));
    let mut removals = Removals::new(&Lost);
    let removal = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
    removals.remove(&Recorder::default(), &removal).unwrap();
    assert_eq!(pane_rows(&state), ["w1:p1"]);

    until("it reported", || {
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );
        removals.is_empty()
    });

    assert_eq!(
        pane_rows(&state),
        ["w1:p1", "w2:p1"],
        "the list really was read again, and not only said to have been"
    );
    assert_eq!(
        state.message(),
        Some(
            "the removal of feat/login ended without saying what happened; the list has \
             been read again since"
        )
    );
}

#[test]
fn an_ending_nobody_could_read_and_a_list_nobody_could_read_are_both_said() {
    // Neither reading got an answer — the child's, and this side's — so the sentence
    // saying the list has been read is not said, and what could not be read is.
    let recorder = Recorder::default();
    let git = Answers(Some(false));
    let mut state = PanesState::new(one_pane_tree(), None);
    let mut pending = pending(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));
    let mut removals = Removals::new(&Lost);
    let removal = only_removal(&state);
    removals.remove(&recorder, &removal).unwrap();

    until("it reported", || {
        drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
        removals.is_empty()
    });

    assert_eq!(state.message(), Some("the removal of feat/login ended without saying what happened — its 1 pane was closed first"));
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
}

#[test]
fn a_removal_that_worked_says_when_the_list_could_not_be_caught_up_with_it() {
    // Saying nothing is the report when it worked, because the row leaves the list. When
    // the list cannot be read again the row does not leave, and saying nothing then says
    // the opposite of what happened.
    let recorder = Recorder::default();
    let git = Answers(Some(false));
    let port = Reports(RemovalOutcome::Removed);
    let mut state = PanesState::new(one_pane_tree(), None);
    let mut pending = pending(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));
    let mut removals = Removals::new(&port);
    let removal = only_removal(&state);
    removals.remove(&recorder, &removal).unwrap();

    until("it reported", || {
        drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
        removals.is_empty()
    });

    assert_eq!(state.message(), None);
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
}

#[test]
fn what_became_of_the_list_is_said_before_the_question_that_went_with_it() {
    // The withdrawal is about the box rather than about any removal, so it stays last;
    // ahead of the sentence about the list it would read as a remark on the list.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = sweep_asked(&session);
    let mut removals = Removals::new(&Lost);
    let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
    removals.remove(&Recorder::default(), &earlier).unwrap();

    until("it reported", || {
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );
        removals.is_empty()
    });

    assert!(
        state.pending_sweep().is_none(),
        "the box went with the reading"
    );
    assert_eq!(
        state.message(),
        Some(
            "the removal of feat/login ended without saying what happened; the list has \
             been read again since; the list changed while that was up — ask again"
        )
    );
}

#[test]
fn a_refusal_reads_the_list_again_too_because_nothing_else_retries_it() {
    // git declined, so the refusal changes nothing by itself — but if `start_removal`'s
    // own reading did not get through when it closed this checkout's panes, rows for
    // stopped panes are on screen and that failure was a sentence the next keypress
    // wiped. `Recorder` will not describe itself, so the reading arrives as a clause.
    let recorder = Recorder::default();
    let git = Answers(Some(false));
    let port = Reports(RemovalOutcome::Refused("fatal: refused".into()));
    let mut state = PanesState::new(one_pane_tree(), None);
    let mut pending = pending(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));
    let mut removals = Removals::new(&port);
    let removal = only_removal(&state);
    removals.remove(&recorder, &removal).unwrap();

    until("it reported", || {
        drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
        removals.is_empty()
    });

    assert_eq!(
        state.message(),
        Some("could not remove feat/login: fatal: refused — its 1 pane was closed first")
    );
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
}

#[test]
fn a_question_over_a_list_this_could_not_read_goes_back_with_it() {
    // `re_read` takes a question back when its reading fails; this is the same situation
    // reached the other way. Left standing, `y` acts on a pane list taken before the
    // removal that just reported — ADR 0010's hazard, `git worktree remove` over a pane
    // nobody closed.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = sweep_asked(&session);
    let port = Reports(RemovalOutcome::BranchKept("error: not fully merged".into()));
    let mut removals = Removals::new(&port);
    let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
    removals.remove(&Recorder::default(), &earlier).unwrap();
    assert!(state.pending_sweep().is_some(), "the box is up");

    // A herdr that will not describe itself takes the frame's one reading.
    let recorder = Recorder::default();
    let git = Answers(Some(false));
    until("it reported", || {
        drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
        removals.is_empty()
    });

    assert!(
        state.pending_sweep().is_none(),
        "the box went with the reading it was waiting on"
    );
    assert_eq!(
        state.message(),
        Some("removed feat/login, branch kept: error: not fully merged; the question went back"),
        "and not `WITHDRAWN`, which would say the list changed when nothing replaced it"
    );
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
}

#[test]
fn two_endings_nobody_could_read_are_settled_by_the_one_list_once() {
    // Said per report it would say the same thing twice about one reading.
    let session = Arc::new(Session::new(false, false));
    let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut pending = pending(Dirty::new(session.clone()));
    let mut removals = Removals::new(&Lost);
    for worktree in &state.tree().repos[0].worktrees[1..=2] {
        let removal = Removal::sweeping("/src/app", worktree);
        removals.remove(&Recorder::default(), &removal).unwrap();
    }
    until("both reported", || removals.reported() == 2);
    drain_finished(
        &mut state,
        &mut pending,
        &mut removals,
        &*session,
        &*session,
    );

    let line = state
        .message()
        .expect("two endings nobody could read are said");
    assert_eq!(
        line.matches(READ_AGAIN).count(),
        1,
        "one list, one sentence: {line:?}"
    );
    assert!(line.ends_with(READ_AGAIN), "and it comes last: {line:?}");
    assert_eq!(
        line.matches("; ").count(),
        2,
        "two reports and the one sentence: {line:?}"
    );
}

#[test]
fn two_reports_over_one_reading_that_failed_say_what_failed_once() {
    // The same rule for the other sentence: one reading was taken, so one thing is said.
    let session = Arc::new(Session::new(false, false));
    let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut pending = pending(Dirty::new(session.clone()));
    let mut removals = Removals::new(&Lost);
    for worktree in &state.tree().repos[0].worktrees[1..=2] {
        let removal = Removal::sweeping("/src/app", worktree);
        removals.remove(&Recorder::default(), &removal).unwrap();
    }
    until("both reported", || removals.reported() == 2);
    let recorder = Recorder::default();
    let git = Answers(Some(false));
    drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);

    let line = state
        .message()
        .expect("two endings nobody could read are said");
    assert!(
        !line.contains(STALE),
        "what became of the list is not one of the reports: {line:?}"
    );
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering"),
        "one list, one sentence — and being a condition is what makes it one, however \
         many removals reported over the reading that failed"
    );
    assert!(
        !line.contains(READ_AGAIN),
        "and not the one that says it was read: {line:?}"
    );
    assert_eq!(
        line.matches("; ").count(),
        1,
        "the two reports, and nothing else on the line: {line:?}"
    );
}

#[test]
fn a_reading_that_worked_takes_back_the_sentence_saying_one_could_not() {
    // Issue #64's sequence, and the reason the list being behind is a condition rather
    // than a push. Two removals from one sweep report in different frames with no key
    // pressed between them, which is the ordinary case — ADR 0014 has the user carrying
    // on while they run. The first frame's reading fails. The second frame's works, and
    // `words::message` has nothing to say about a removal that simply worked, so there
    // is no second sentence to write over the first: what retires it has to be the
    // reading itself.
    let session = Arc::new(Session::new(false, false));
    let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut pending = pending(Dirty::new(session.clone()));
    // The session answers as both, because the second frame's reading has to work.
    let git = session.clone();

    let mut lost = Removals::new(&Lost);
    let first = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
    lost.remove(&Recorder::default(), &first).unwrap();
    until("the first reported", || lost.reported() == 1);
    // A herdr that will not describe itself takes this frame's one reading.
    drain_finished(
        &mut state,
        &mut pending,
        &mut lost,
        &Recorder::default(),
        &*git,
    );
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering"),
        "the rows are behind, and the line says so"
    );

    let mut removed = Removals::new(&Reports(RemovalOutcome::Removed));
    let second = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[2]);
    removed.remove(&Recorder::default(), &second).unwrap();
    until("the second reported", || removed.reported() == 1);
    drain_finished(&mut state, &mut pending, &mut removed, &*session, &*git);

    assert_eq!(
        state.trouble(),
        None,
        "and the reading that put the list right is what takes the sentence back"
    );
    assert!(
        !state.message().unwrap_or_default().contains(STALE),
        "with nothing else on screen still saying it either: {:?}",
        state.message()
    );
}

#[test]
fn a_sweeps_enter_still_waiting_for_its_box_goes_back_too() {
    // Between `Enter` and the box the question lives in the sweep rather than in a
    // `pending_*`, because `confirm_sweep_if_settled` will not put a box up over a walk
    // that has not answered. Missed, the user's `Enter` is eaten — no box ever comes,
    // the marks stay on screen, and the line says nothing about the sweep.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = enter_pressed(&session);
    assert!(
        state.pending_sweep().is_none(),
        "the box is waiting on the walk"
    );
    let mut removals = Removals::new(&Lost);
    let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
    removals.remove(&Recorder::default(), &earlier).unwrap();

    // This arm formats the error itself rather than going through `could_not_read`, so
    // it is a second place the whole chain has to reach the line.
    let git = Answers(Some(false));
    until("it reported", || {
        drain_finished(&mut state, &mut pending, &mut removals, &Chained, &git);
        removals.is_empty()
    });

    assert_eq!(state.message(), Some("the removal of feat/login ended without saying what happened; nothing was asked"), "the `Enter` went, and the line says which of the two it was: nothing was asked, so the marks stand and `Enter` again is the whole of the way back");
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: sending session.snapshot to herdr: broken pipe")
    );
    settle(&mut state, &mut pending);
    assert!(
        state.pending_sweep().is_none(),
        "and no box comes up afterwards over the list nobody could read"
    );
}

#[test]
fn a_shift_d_box_over_a_list_this_could_not_read_goes_back_too() {
    // `pending_removal` and `pending_sweep` are never up together, so the sweep's test
    // cannot stand in for this one.
    let mut state = PanesState::new(one_pane_tree(), None);
    let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
    dirty.ask(state.tree());
    let mut pending = pending(dirty);
    until("the walk answered", || {
        !show_answers(&mut state, &mut pending)
    });
    press(&mut state, 'D');
    assert!(state.pending_removal().is_some(), "the box is up");

    let recorder = Recorder::default();
    let git = Answers(Some(false));
    let mut removals = Removals::new(&Lost);
    let removal = only_removal(&state);
    removals.remove(&recorder, &removal).unwrap();
    until("it reported", || {
        drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
        removals.is_empty()
    });

    assert!(state.pending_removal().is_none(), "and it went back");
    assert_eq!(state.message(), Some("the removal of feat/login ended without saying what happened — its 1 pane was closed first; the question went back"));
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: herdr is not answering")
    );
}

#[test]
fn a_checkout_that_left_the_list_is_forgotten_by_the_walk() {
    // `catch_up` hands the new tree to `Dirty`, which is the only thing that scopes the
    // walk to what is there now. Dropped, a checkout that arrives is never walked at all,
    // so its row answers "still reading that working tree" and can never be removed.
    let herdr = Arc::new(Closing::where_the_checkout_goes_too());
    let recorder = Recorder::default();
    let port = Started(&recorder);
    let mut removals = Removals::new(&port);
    let (_, tree) = collect::collect_tree(&*herdr, &*herdr).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut dirty = Dirty::new(herdr.clone());
    dirty.ask(state.tree());
    until("both checkouts were walked", || {
        dirty.drain();
        dirty.answers().len() == 2
    });
    let removal = removal_of(&state, FEAT_LOGIN);

    start_removal(
        &mut state,
        &mut dirty,
        &mut removals,
        &*herdr,
        &*herdr,
        &removal,
    );

    assert_eq!(
        dirty.answers().keys().collect::<Vec<_>>(),
        [&CheckoutPath::for_test("/src/app")],
        "the checkout that went is not one the walk still has an answer about, and the \
         one that stayed is"
    );
}

#[test]
fn what_herdr_said_reaches_the_line_with_its_cause_under_it() {
    // The socket adapter wraps: `sending session.snapshot to herdr` over the io error
    // that actually happened. Without the alternate form the line carries the wrapper
    // and drops the half that says what went wrong.
    let recorder = Recorder::default();
    let port = Started(&recorder);
    let mut removals = Removals::new(&port);
    let mut state = PanesState::new(one_pane_tree(), None);
    let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
    let removal = only_removal(&state);

    start_removal(
        &mut state,
        &mut dirty,
        &mut removals,
        &Chained,
        &Answers(Some(false)),
        &removal,
    );

    assert_eq!(state.message(), None);
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: sending session.snapshot to herdr: broken pipe")
    );
}

#[test]
fn a_frame_with_nothing_to_take_in_leaves_the_list_alone() {
    // A herdr that will not describe itself, so a reading nobody asked for arrives as a
    // sentence rather than as silence. Without the guard the picker reads the whole
    // session on every tick a removal is in flight for, and a box goes back on the frame
    // after it came up.
    let recorder = Recorder::default();
    let git = Answers(Some(false));
    let mut state = PanesState::new(one_pane_tree(), None);
    let mut pending = pending(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));
    let mut removals = Removals::new(&Refuses);

    drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);

    assert_eq!(
        state.message(),
        None,
        "nothing reported, so nothing was read"
    );
}

#[test]
fn a_removal_that_worked_over_a_list_that_read_says_nothing_at_all() {
    // The contract ADR 0014 and the changelog state: the row leaving the list is the
    // report. It holds only while the list keeps up.
    let herdr = Arc::new(Closing::where_the_checkout_goes_too());
    let port = Reports(RemovalOutcome::Removed);
    let mut removals = Removals::new(&port);
    let (_, tree) = collect::collect_tree(&*herdr, &*herdr).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut pending = pending(Dirty::new(herdr.clone()));
    let removal = removal_of(&state, FEAT_LOGIN);
    removals.remove(&*herdr, &removal).unwrap();

    until("it reported", || {
        drain_finished(&mut state, &mut pending, &mut removals, &*herdr, &*herdr);
        removals.is_empty()
    });

    assert_eq!(state.message(), None, "the row leaving is the whole report");
    assert_eq!(
        pane_rows(&state),
        ["w1:p9"],
        "and it left: the panes that were in it are off the list"
    );
}

#[test]
fn a_sweeps_enter_still_waiting_survives_a_reading_that_worked() {
    // `replace_tree` leaves `confirming` alone on purpose — the reading it waits on is
    // the one that just happened — so an unrelated removal must not cost an `Enter`.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = enter_pressed(&session);
    let port = Reports(RemovalOutcome::Removed);
    let mut removals = Removals::new(&port);
    let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
    removals.remove(&Recorder::default(), &earlier).unwrap();

    until("it reported", || {
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );
        removals.is_empty()
    });

    settle(&mut state, &mut pending);
    assert!(
        !box_paths(&state).is_empty(),
        "the box the `Enter` asked for still comes up: {:?}",
        state.message()
    );
}

#[test]
fn a_reading_that_failed_reaches_the_line_with_its_cause_under_it_too() {
    // `Chained` is the only fake here whose refusal has a cause under it, and this arm
    // formats the error itself rather than going through `could_not_read`.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = sweep_asked(&session);
    let mut removals = Removals::new(&Lost);
    let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
    removals.remove(&Recorder::default(), &earlier).unwrap();

    let git = Answers(Some(false));
    until("it reported", || {
        drain_finished(&mut state, &mut pending, &mut removals, &Chained, &git);
        removals.is_empty()
    });

    assert_eq!(
        state.message(),
        Some(
            "the removal of feat/login ended without saying what happened; the question went back"
        )
    );
    assert_eq!(
        state.trouble().as_deref(),
        Some("the list could not be read again: sending session.snapshot to herdr: broken pipe")
    );
}

#[test]
fn a_removal_that_could_not_be_waited_on_reaches_the_line_whole() {
    // `Detached::wait`'s other failure, where the wait itself fails. Both halves are the
    // report: the wrapper says which removal, the cause says why nobody can tell.
    let session = Arc::new(Session::new(false, false));
    let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut pending = pending(Dirty::new(session.clone()));
    let mut removals = Removals::new(&Unwaited);
    let removal = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
    removals.remove(&Recorder::default(), &removal).unwrap();

    until("it reported", || {
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );
        removals.is_empty()
    });

    assert_eq!(
        state.message(),
        Some(
            "waiting for the removal of feat/login: No child processes (os error 10); \
             the list has been read again since"
        )
    );
}

#[test]
fn one_ending_nobody_could_read_among_reports_that_worked_is_settled_once() {
    // `unknown` is a fact about the frame, not about a report, so the sentence lands
    // after both and is said once.
    let session = Arc::new(Session::new(false, false));
    let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
    let mut state = PanesState::new(tree, None);
    let mut pending = pending(Dirty::new(session.clone()));
    let port = LostFirst::default();
    let mut removals = Removals::new(&port);
    for worktree in &state.tree().repos[0].worktrees[1..=2] {
        let removal = Removal::sweeping("/src/app", worktree);
        removals.remove(&Recorder::default(), &removal).unwrap();
    }
    until("both reported", || removals.reported() == 2);
    drain_finished(
        &mut state,
        &mut pending,
        &mut removals,
        &*session,
        &*session,
    );

    let line = state
        .message()
        .expect("the ending nobody could read is said");
    assert_eq!(
        line,
        "the removal of feat/login ended without saying what happened; the list has \
         been read again since",
        "the one that worked says nothing, and the list is settled once, last"
    );
}

#[test]
fn a_refused_removal_stops_its_row_spinning_when_it_reports() {
    // The `deleting` note and its spinner are for a process still running.
    let session = Arc::new(Session::new(false, false));
    let (mut state, mut pending) = sweep_asked(&session);
    let port = Reports(RemovalOutcome::Refused("fatal: refused".into()));
    let mut removals = Removals::new(&port);
    let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
        panic!("`y` is the answer that goes ahead");
    };
    start_sweep(&mut state, &mut removals, &Recorder::default(), &sweep);
    assert!(state.rows().iter().any(|row| row.is_removing));
    until("both reported", || {
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );
        removals.is_empty()
    });

    assert!(
        !state.rows().iter().any(|row| row.is_removing),
        "a refused row is an ordinary row again: {:?}",
        state.message()
    );
}
