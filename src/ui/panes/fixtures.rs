//! The tree, the keys and the readings these tests are all written against.
//!
//! Here rather than in one submodule's `mod tests` because the same fixture is what the
//! list, the sweep and the keymap are each asked about.

use std::collections::BTreeMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::domain::model::{
    Branch, CheckoutPath, PaneNode, Refs, RepoKey, RepoNode, Tree, WorkingTree, WorktreeNode,
};
use crate::domain::rows::DisplayLine;
use crate::domain::sweep::Mark;
use crate::port::AgentStatus;
use crate::port::Track;
use crate::ui::panes::{Action, PanesState};
use crate::ui::words;

/// The key a checkout of the fixture's one repository is judged under.
pub(crate) fn at(state: &PanesState, path: &str) -> (RepoKey, CheckoutPath) {
    (
        RepoKey::of(&state.tree.repos[0]),
        CheckoutPath::for_test(path),
    )
}

/// The answers map, spelled out per checkout: these tests care which of the four shapes
/// a checkout is in.
pub(crate) fn answers(pairs: &[(&str, WorkingTree)]) -> BTreeMap<CheckoutPath, WorkingTree> {
    pairs
        .iter()
        .map(|(path, answer)| (CheckoutPath::for_test(path), *answer))
        .collect()
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(crate) fn pane(id: &str, name: &str, status: AgentStatus) -> PaneNode {
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
pub(crate) fn state() -> PanesState {
    PanesState::new(
        Tree {
            repos: vec![RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                refs: Refs::Read,
                worktrees: vec![
                    WorktreeNode {
                        branch: Branch::Out("main".into()),
                        checkout_path: "/src/app".into(),
                        is_primary: true,
                        open_workspace_id: Some("w1".into()),
                        track: None,
                        panes: vec![pane("w1:p1", "claude", AgentStatus::Working)],
                    },
                    WorktreeNode {
                        branch: Branch::Out("feat/login".into()),
                        checkout_path: "/wt/app/feat-login".into(),
                        is_primary: false,
                        open_workspace_id: Some("w2".into()),
                        track: None,
                        panes: vec![pane("w2:p1", "codex", AgentStatus::Blocked)],
                    },
                    WorktreeNode {
                        branch: Branch::Out("fix/crash".into()),
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
pub(crate) fn select(state: &mut PanesState, label: &str) {
    let index = state
        .rows()
        .iter()
        .position(|row| words::row_label(row) == label)
        .unwrap_or_else(|| panic!("no row labelled {label}"));
    state.cursor = state
        .lines()
        .iter()
        .position(|line| *line == DisplayLine::Row(index))
        .unwrap();
}

pub(crate) fn cursor_label(state: &PanesState) -> String {
    match state.lines()[state.cursor] {
        DisplayLine::Row(index) => words::row_label(&state.rows()[index]),
        DisplayLine::Spacer => panic!("the cursor is on a spacer"),
    }
}

pub(crate) fn row_labels(state: &PanesState) -> Vec<String> {
    state.rows().iter().map(words::row_label).collect()
}

/// A sweep with git's answers in place, and one checkout in each shape the sweep can
/// see: `main` is the repository's own, `feat/login` is clean and nobody is finished
/// with it, `fix/crash` has a gone upstream, and `feat/wip` has an agent in it.
///
/// `feat/login`'s pane goes: a checkout with panes in it is refused before anything else
/// is asked about it, which would leave the interesting half of these tests unreachable.
pub(crate) fn sweeping() -> PanesState {
    let mut state = state();
    state.tree.repos[0].worktrees[1].panes.clear();
    state.tree.repos[0].worktrees[1].open_workspace_id = None;
    state.tree.repos[0].worktrees[2].track = Some(Track::Gone);
    state.tree.repos[0].worktrees.push(WorktreeNode {
        branch: Branch::Out("feat/wip".into()),
        checkout_path: "/wt/app/feat-wip".into(),
        is_primary: false,
        open_workspace_id: Some("w3".into()),
        track: Some(Track::Gone),
        panes: vec![pane("w3:p1", "codex", AgentStatus::Working)],
    });
    state.replace_tree(state.tree.clone());
    state.set_working_trees(answers(&[
        ("/src/app", WorkingTree::Clean),
        ("/wt/app/feat-login", WorkingTree::Clean),
        ("/wt/app/fix-crash", WorkingTree::Clean),
        ("/wt/app/feat-wip", WorkingTree::Clean),
    ]));
    assert_eq!(state.handle_key(key(KeyCode::Char('S'))), Action::Consumed);
    state
}

pub(crate) fn mark_of(state: &PanesState, label: &str) -> Option<Mark> {
    state
        .rows()
        .iter()
        .find(|row| words::row_label(row) == label)
        .unwrap_or_else(|| panic!("no row labelled {label}"))
        .sweep
        .clone()
}

/// `Enter`, and the frame on which the walk it asked for has answered.
pub(crate) fn ask(state: &mut PanesState) -> Action {
    let action = state.handle_key(key(KeyCode::Enter));
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    action
}

/// What the sweep's box lists, by label.
pub(crate) fn box_labels(state: &PanesState) -> Vec<&str> {
    state
        .pending_sweep()
        .map(|sweep| sweep.removals().iter().map(|r| r.label()).collect())
        .unwrap_or_default()
}
