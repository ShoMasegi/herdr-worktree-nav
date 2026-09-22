//! What the snapshot tests in this module drive the pickers with.
//!
//! Here rather than in one submodule's `mod tests` because the same tree, theme and buffer
//! readers serve all three: a screen is drawn from one state and read back one cell at a
//! time, and the readers are the same whichever picker drew it.

use crate::domain::model::{CheckoutPath, WorkingTree};
use crate::ui::render::{branches, panes};
use std::collections::BTreeMap;

use ratatui::style::Style;

use crate::domain::chrome::Chrome;
use crate::domain::dest::fixtures::{space_name, tab_name};
use crate::domain::dest::Destination;
use crate::domain::model::{PaneNode, Refs, RepoNode, Tree, WorktreeNode};
use crate::domain::rows::DisplayLine;
use crate::port::{AgentStatus, GitRef, PullRequest, RefKind, SplitDirection, Track};
use crate::ui::branches::BranchData;
use crate::ui::branches::BranchesState;
use crate::ui::state::PanesState;
use crate::ui::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;

/// The answers map, spelled out per checkout.
pub(crate) fn answers(pairs: &[(&str, WorkingTree)]) -> BTreeMap<CheckoutPath, WorkingTree> {
    pairs
        .iter()
        .map(|(path, answer)| (CheckoutPath::for_test(path), *answer))
        .collect()
}

pub(crate) fn theme() -> Theme {
    Theme::new(Chrome::default())
}

pub(crate) fn pane(id: &str, name: Option<&str>, status: AgentStatus, focused: bool) -> PaneNode {
    let workspace = id.split(':').next().unwrap().to_string();
    PaneNode {
        pane_id: id.into(),
        tab_id: format!("{workspace}:t1"),
        workspace_id: workspace,
        display_name: name.map(str::to_string),
        agent_status: status,
        focused,
    }
}

pub(crate) fn worktree(branch: &str, primary: bool, panes: Vec<PaneNode>) -> WorktreeNode {
    WorktreeNode {
        branch: Some(branch.into()),
        checkout_path: format!("/wt/{}", branch.replace('/', "-")),
        is_primary: primary,
        open_workspace_id: panes.first().map(|p| p.workspace_id.clone()),
        track: None,
        panes,
    }
}

pub(crate) fn tree() -> Tree {
    Tree {
        repos: vec![
            RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                refs: Refs::Read,
                worktrees: vec![
                    worktree(
                        "main",
                        true,
                        vec![
                            pane("w1:p1", Some("claude"), AgentStatus::Working, true),
                            pane("w1:p2", None, AgentStatus::Unknown, false),
                        ],
                    ),
                    worktree(
                        "feat/login",
                        false,
                        vec![pane("w2:p1", Some("codex"), AgentStatus::Idle, false)],
                    ),
                    worktree("fix/crash", false, vec![]),
                ],
            },
            RepoNode {
                repo_key: "/src/site/.git".into(),
                repo_root: "/src/site".into(),
                display_name: "me/site".into(),
                refs: Refs::Read,
                worktrees: vec![worktree(
                    "develop",
                    true,
                    vec![pane("w3:p1", Some("claude"), AgentStatus::Blocked, false)],
                )],
            },
        ],
        ungrouped: vec![pane("w9:p1", None, AgentStatus::Unknown, false)],
    }
}

pub(crate) fn press(state: &mut PanesState, code: KeyCode) {
    state.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

pub(crate) fn screen(state: &PanesState, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            panes::draw(frame, state, &theme());
        })
        .unwrap();
    terminal.backend().to_string()
}

/// What [`adapter::git_cli::local_refs`] actually hands up when a loose ref is broken:
/// git's words, and only those — the call is left off this one sentence, for the reason
/// `dropped_refs` gives.
pub(crate) const REFS_REFUSAL: &str = "warning: ignoring broken ref refs/heads/main";

/// And what it hands up when git dropped two, which is one warning per ref joined with a
/// space.
pub(crate) const TWO_REFS_REFUSAL: &str = "warning: ignoring broken ref refs/heads/main \
    warning: ignoring broken ref refs/heads/chore/deps";

pub(crate) fn prompt_line(state: &PanesState, width: u16) -> String {
    let drawn = screen(state, width, 18);
    drawn
        .lines()
        .next()
        .expect("the prompt line")
        .trim_matches('"')
        .to_string()
}

pub(crate) fn panes_help(width: u16) -> &'static str {
    super::first_that_fits(panes::HELP_PANES, width)
}

/// The panes tree with a sweep on, one checkout in each shape it can be in: going,
/// staying, refused for a pane, refused for being the repository itself.
pub(crate) fn swept() -> PanesState {
    let mut tree = tree();
    // Finished with: gone upstream, nothing running in it.
    tree.repos[0].worktrees[2].track = Some(Track::Gone);
    // Gone too, but somebody is working in it.
    tree.repos[0].worktrees[1].track = Some(Track::Gone);
    // Nobody is finished with this one, and the user may still say otherwise.
    tree.repos[1]
        .worktrees
        .push(worktree("chore/deps", false, vec![]));
    let mut state = PanesState::new(tree, None);
    state.set_working_trees(answers(&[
        ("/wt/main", WorkingTree::Clean),
        ("/wt/feat-login", WorkingTree::Clean),
        ("/wt/fix-crash", WorkingTree::Clean),
        ("/wt/develop", WorkingTree::Clean),
        ("/wt/chore-deps", WorkingTree::Clean),
    ]));
    press(&mut state, KeyCode::Char('S'));
    state
}

/// The style of one cell. `screen()` serialises characters and throws every style away,
/// so a snapshot says nothing about whether a `[x]` is drawn as a mark or as chrome.
pub(crate) fn cell_style(state: &PanesState, width: u16, height: u16, x: u16, y: u16) -> Style {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            panes::draw(frame, state, &theme());
        })
        .unwrap();
    terminal.backend().buffer()[(x, y)].style()
}

pub(crate) fn label_under_cursor(state: &PanesState) -> String {
    match state.lines()[state.cursor()] {
        crate::domain::rows::DisplayLine::Row(index) => state.rows()[index].label.clone(),
        crate::domain::rows::DisplayLine::Spacer => String::new(),
    }
}

pub(crate) fn line_of(state: &PanesState, label: &str) -> u16 {
    let index = state
        .rows()
        .iter()
        .position(|row| row.label == label)
        .unwrap_or_else(|| panic!("no row labelled {label}"));
    let at = state
        .lines()
        .iter()
        .position(|line| *line == DisplayLine::Row(index))
        .expect("the row is drawn");
    at as u16 + 2
}

/// One repository: the session's own checkout with a pane in it, and `finished` linked
/// worktrees, each on a branch whose upstream is gone and with nothing running in it.
pub(crate) fn finished_tree(finished: &[&str]) -> Tree {
    let mut worktrees = vec![worktree(
        "main",
        true,
        vec![pane("w1:p1", Some("claude"), AgentStatus::Working, true)],
    )];
    worktrees.extend(finished.iter().map(|branch| WorktreeNode {
        track: Some(Track::Gone),
        ..worktree(branch, false, vec![])
    }));
    Tree {
        repos: vec![RepoNode {
            repo_key: "/src/app/.git".into(),
            repo_root: "/src/app".into(),
            display_name: "me/app".into(),
            refs: Refs::Read,
            worktrees,
        }],
        ungrouped: vec![],
    }
}

/// A sweep over `tree` with every working tree clean.
pub(crate) fn sweeping_over(tree: Tree) -> PanesState {
    let clean: BTreeMap<CheckoutPath, WorkingTree> = tree.repos[0]
        .worktrees
        .iter()
        .map(|worktree| (CheckoutPath::of(worktree), WorkingTree::Clean))
        .collect();
    let mut state = PanesState::new(tree, None);
    state.set_working_trees(clean);
    press(&mut state, KeyCode::Char('S'));
    state
}

/// `Enter`, and the frame on which the walk it asked for has answered.
pub(crate) fn ask(state: &mut PanesState) {
    press(state, KeyCode::Enter);
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert!(state.pending_sweep().is_some(), "a question should be up");
}

/// The style of the cell where `needle` starts, on the first row containing it.
///
/// Column, not byte offset: the rows are full of box-drawing and status glyphs, so
/// [`str::find`] would land several cells to the right of the label.
pub(crate) fn style_of_row(buffer: &ratatui::buffer::Buffer, needle: &str) -> Style {
    let area = buffer.area();
    for y in area.y..area.y + area.height {
        let cells: Vec<String> = (area.x..area.x + area.width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect();
        let line: String = cells.concat();
        if !line.contains(needle) {
            continue;
        }
        let column = (0..cells.len())
            .find(|start| cells[*start..].concat().starts_with(needle))
            .expect("the needle starts at some cell");
        return buffer[(area.x + column as u16, y)].style();
    }
    panic!("no row containing {needle}");
}

pub(crate) fn buffer_of(state: &PanesState, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            panes::draw(frame, state, &theme());
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

pub(crate) fn git_ref(name: &str, at: i64) -> GitRef {
    GitRef {
        name: name.into(),
        kind: RefKind::Local,
        committed_at: Some(at),
        subject: Some(format!("latest work on {name}")),
        upstream: None,
        track: None,
        worktree_path: None,
    }
}

/// The picker as it opens with two repositories in the session: on its repository
/// step, with `me/app` — where it was summoned from — under the cursor.
pub(crate) fn branches_picker() -> BranchesState {
    let repo = RepoNode {
        repo_key: "/src/app/.git".into(),
        repo_root: "/src/app".into(),
        display_name: "me/app".into(),
        refs: Refs::Read,
        worktrees: vec![
            worktree(
                "feat/login",
                false,
                vec![pane("w2:p1", Some("claude"), AgentStatus::Working, false)],
            ),
            worktree("fix/crash", false, vec![]),
        ],
    };
    let other = RepoNode {
        repo_key: "/home/me/src/notes/.git".into(),
        repo_root: "/home/me/src/notes".into(),
        display_name: "me/notes".into(),
        refs: Refs::Read,
        worktrees: vec![worktree(
            "main",
            true,
            vec![pane("w3:p1", None, AgentStatus::Unknown, false)],
        )],
    };
    let snapshot: crate::port::Snapshot = serde_json::from_value(serde_json::json!({
        "version": "0.7.4",
        "protocol": 16,
        "workspaces": [
            {"workspace_id": "w1", "label": "app", "number": 1, "focused": true,
             "active_tab_id": "w1:t1", "agent_status": "idle"},
        ],
        "tabs": [
            {"tab_id": "w1:t1", "workspace_id": "w1", "label": "agents", "number": 1,
             "focused": true, "pane_count": 2, "agent_status": "idle"},
            {"tab_id": "w3:t1", "workspace_id": "w3", "label": "zoomed", "number": 1,
             "focused": false, "pane_count": 1, "agent_status": "idle"},
        ],
        "panes": [
            {"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1",
             "terminal_id": "t1", "focused": true, "agent": "claude",
             "agent_status": "working"},
            {"pane_id": "w1:p9", "tab_id": "w1:t1", "workspace_id": "w1",
             "terminal_id": "t9", "focused": false, "agent_status": "unknown"},
        ],
        "layouts": [
            {"tab_id": "w1:t1", "workspace_id": "w1", "zoomed": false,
             "area": {"x": 0, "y": 0, "width": 250, "height": 79},
             "focused_pane_id": "w1:p1",
             "panes": [
                 {"pane_id": "w1:p1", "focused": true,
                  "rect": {"x": 0, "y": 0, "width": 250, "height": 40}},
                 {"pane_id": "w1:p9", "focused": false,
                  "rect": {"x": 0, "y": 40, "width": 250, "height": 39}}
             ]},
            {"tab_id": "w3:t1", "workspace_id": "w3", "zoomed": true,
             "area": {"x": 0, "y": 0, "width": 250, "height": 79},
             "focused_pane_id": "w3:p1",
             "panes": [{"pane_id": "w3:p1", "focused": true,
                        "rect": {"x": 0, "y": 0, "width": 250, "height": 79}}]},
        ],
    }))
    .expect("snapshot fixture should deserialize");
    BranchesState::new(
        vec![repo, other],
        Some("/src/app"),
        vec![
            Destination::SplitHere {
                tab_id: "w1:t1".into(),
                target_pane_id: "w1:p1".into(),
                direction: SplitDirection::Right,
            },
            Destination::SplitHere {
                tab_id: "w1:t1".into(),
                target_pane_id: "w1:p1".into(),
                direction: SplitDirection::Down,
            },
            Destination::ExistingTab {
                tab: tab_name("w1", "app", "w1:t2", "logs"),
                zoomed: false,
            },
            Destination::ExistingSpace {
                space: space_name("w3", "notes"),
            },
            Destination::NewSpace,
            Destination::ExistingTab {
                tab: tab_name("w3", "notes", "w3:t1", "zoomed"),
                zoomed: true,
            },
        ],
        snapshot,
        Some("/home/me".into()),
    )
}

/// The same picker, moved on into `me/app`'s branches.
pub(crate) fn branch_data() -> BranchData {
    BranchData {
        local_refs: vec![
            git_ref("feat/login", 30),
            git_ref("fix/crash", 20),
            git_ref("main", 40),
            git_ref("chore/deps", 10),
        ],
        remote_heads: vec!["feat/search".into(), "main".into()],
        pull_requests: vec![PullRequest {
            number: 123,
            title: "Add the login screen".into(),
            head_ref: "feat/login".into(),
            is_draft: true,
        }],
        loading: false,
        fetching: false,
    }
}

pub(crate) fn branches_state() -> BranchesState {
    let mut state = branches_picker();
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    state.set_data(branch_data());
    state
}

/// Open the search box and type into it: letters are commands until `/` is pressed.
pub(crate) fn search(state: &mut BranchesState, text: &str) {
    state.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in text.chars() {
        state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

pub(crate) fn branches_screen(state: &BranchesState, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| branches::draw(frame, state, &theme()))
        .unwrap();
    terminal.backend().to_string()
}
