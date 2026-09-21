//! What the tests in this module drive the picker with.
//!
//! Here rather than in one submodule's `mod tests` because a key test, a list test and a
//! detail test all start from the same picker over the same two repositories.

use crate::domain::dest::Destination;
use crate::domain::model::RepoNode;
use crate::domain::progress::Stage;
use crate::port::{GitRef, Snapshot};
use crate::ui::branches::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::domain::model::{PaneNode, Refs, WorktreeNode};
use crate::port::{AgentStatus, RefKind, SplitDirection};

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(crate) fn ctrl(state: &mut BranchesState, c: char) -> BranchAction {
    state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

pub(crate) fn type_in(state: &mut BranchesState, text: &str) {
    for c in text.chars() {
        state.handle_key(key(KeyCode::Char(c)));
    }
}

/// Open the search box and type into it.
pub(crate) fn search(state: &mut BranchesState, text: &str) {
    state.handle_key(key(KeyCode::Char('/')));
    assert!(state.is_filtering(), "`/` should have taken the keyboard");
    type_in(state, text);
}

pub(crate) fn local(name: &str, at: i64) -> GitRef {
    GitRef {
        name: name.into(),
        kind: RefKind::Local,
        committed_at: Some(at),
        subject: Some(format!("work on {name}")),
        upstream: None,
        track: None,
        worktree_path: None,
    }
}

pub(crate) fn repo() -> RepoNode {
    RepoNode {
        repo_key: "/src/app/.git".into(),
        repo_root: "/src/app".into(),
        display_name: "me/app".into(),
        refs: Refs::Read,
        worktrees: vec![WorktreeNode {
            branch: Some("feat/live".into()),
            checkout_path: "/wt/feat-live".into(),
            is_primary: false,
            open_workspace_id: Some("w2".into()),
            track: None,
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

pub(crate) fn other_repo() -> RepoNode {
    RepoNode {
        repo_key: "/src/tools/.git".into(),
        repo_root: "/src/tools".into(),
        display_name: "me/tools".into(),
        refs: Refs::Read,
        worktrees: vec![WorktreeNode {
            branch: Some("main".into()),
            checkout_path: "/src/tools".into(),
            is_primary: true,
            open_workspace_id: Some("w5".into()),
            track: None,
            panes: vec![],
        }],
    }
}

pub(crate) fn destinations() -> Vec<Destination> {
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
pub(crate) fn snapshot() -> Snapshot {
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
pub(crate) fn app_branches() -> BranchData {
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
pub(crate) fn state() -> BranchesState {
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
pub(crate) fn two_repos(from: &str) -> BranchesState {
    BranchesState::new(
        vec![other_repo(), repo()],
        Some(from),
        destinations(),
        snapshot(),
        None,
    )
}

pub(crate) fn names(state: &BranchesState) -> Vec<String> {
    state.rows().iter().map(|e| e.name.clone()).collect()
}

pub(crate) fn repo_names(state: &BranchesState) -> Vec<String> {
    state
        .repo_rows()
        .iter()
        .map(|row| row.repo.display_name.clone())
        .collect()
}

pub(crate) fn under_cursor(state: &BranchesState) -> String {
    state.rows()[state.cursor()].name.clone()
}

/// The picker with a branch and a destination chosen, mid-fetch.
pub(crate) fn fetching() -> BranchesState {
    let mut state = state();
    search(&mut state, "chore");
    state.handle_key(key(KeyCode::Enter));
    state.start_working(Stage::Starting {
        branch: "chore/deps".into(),
    });
    state.set_stage(Stage::Fetching {
        remote: "origin".into(),
        branch: "chore/deps".into(),
    });
    state
}
