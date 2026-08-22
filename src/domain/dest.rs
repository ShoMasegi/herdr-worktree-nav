//! Where a worktree pane should be put, and what that means in herdr terms.
//!
//! herdr's `worktree.create` always materialises a whole new workspace — there is no option
//! to place it into an existing tab. So every destination other than "a new space" is
//! reached by creating the worktree and then moving its root pane.
//!
//! Verified against herdr 0.7.4: `pane.move` closes the emptied tab and workspace itself and
//! reports them as `closed_tab_id` / `closed_workspace_id`, and the checkout is left alone.
//! Nothing has to call `workspace.close`, and nothing has to reimplement herdr's worktree
//! placement. See `docs/adr/0001-delegate-worktree-creation.md`.

use crate::port::{PaneDestination, Snapshot, SplitDirection, Tab, Workspace};

/// One row of the destination step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Split the pane the picker was summoned from.
    SplitHere {
        tab_id: String,
        target_pane_id: String,
        direction: SplitDirection,
    },
    /// Split some other tab, at whichever pane herdr picks.
    ExistingTab { tab_id: String, label: String },
    /// Add a tab to an existing space.
    ExistingSpace { workspace_id: String, label: String },
    /// Leave the workspace herdr made — the built-in behaviour.
    NewSpace,
}

impl Destination {
    /// What the row reads as in the picker.
    pub fn label(&self) -> String {
        match self {
            Destination::SplitHere { direction, .. } => {
                format!("split {}", direction.as_str())
            }
            Destination::ExistingTab { label, .. } => label.clone(),
            Destination::ExistingSpace { label, .. } => label.clone(),
            Destination::NewSpace => "open as a new space".to_string(),
        }
    }

    /// A heading for the group this row belongs to, when it starts one.
    pub fn group(&self) -> &'static str {
        match self {
            Destination::SplitHere { .. } => "here",
            Destination::ExistingTab { .. } => "existing tab",
            Destination::ExistingSpace { .. } => "existing space",
            Destination::NewSpace => "",
        }
    }
}

/// Where to move the freshly created root pane, or `None` to leave it where herdr put it.
pub fn placement_for(destination: &Destination) -> Option<PaneDestination> {
    match destination {
        Destination::SplitHere {
            tab_id,
            target_pane_id,
            direction,
        } => Some(PaneDestination::Tab {
            tab_id: tab_id.clone(),
            split: *direction,
            target_pane_id: Some(target_pane_id.clone()),
        }),
        Destination::ExistingTab { tab_id, .. } => Some(PaneDestination::Tab {
            tab_id: tab_id.clone(),
            split: SplitDirection::Right,
            // No target: herdr splits the tab's active pane.
            target_pane_id: None,
        }),
        Destination::ExistingSpace { workspace_id, .. } => Some(PaneDestination::NewTab {
            workspace_id: Some(workspace_id.clone()),
        }),
        Destination::NewSpace => None,
    }
}

/// Build the destination list for a picker summoned from `from_pane_id`.
///
/// `exclude_pane_id` is the picker's own pane: splitting the overlay the user is looking at
/// is never what they meant.
pub fn destinations(
    snapshot: &Snapshot,
    from_pane_id: Option<&str>,
    exclude_pane_id: Option<&str>,
) -> Vec<Destination> {
    let mut destinations = Vec::new();

    let origin = from_pane_id
        .filter(|id| Some(*id) != exclude_pane_id)
        .and_then(|id| snapshot.panes.iter().find(|pane| pane.pane_id == id));

    if let Some(pane) = origin {
        for direction in [SplitDirection::Right, SplitDirection::Down] {
            destinations.push(Destination::SplitHere {
                tab_id: pane.tab_id.clone(),
                target_pane_id: pane.pane_id.clone(),
                direction,
            });
        }
    }

    // The tab the picker came from is already covered by "split here".
    let current_tab = origin.map(|pane| pane.tab_id.as_str());
    for tab in &snapshot.tabs {
        if Some(tab.tab_id.as_str()) == current_tab {
            continue;
        }
        destinations.push(Destination::ExistingTab {
            tab_id: tab.tab_id.clone(),
            label: tab_label(snapshot, tab),
        });
    }

    for workspace in &snapshot.workspaces {
        destinations.push(Destination::ExistingSpace {
            workspace_id: workspace.workspace_id.clone(),
            label: format!("{} \u{2192} new tab", workspace_label(workspace)),
        });
    }

    destinations.push(Destination::NewSpace);
    destinations
}

fn workspace_label(workspace: &Workspace) -> String {
    if workspace.label.trim().is_empty() {
        workspace.workspace_id.clone()
    } else {
        format!("{}  {}", workspace.workspace_id, workspace.label.trim())
    }
}

fn tab_label(snapshot: &Snapshot, tab: &Tab) -> String {
    let space = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == tab.workspace_id)
        .map(workspace_label)
        .unwrap_or_else(|| tab.workspace_id.clone());
    if tab.label.trim().is_empty() {
        format!("{space} / {}", tab.tab_id)
    } else {
        format!("{space} / {}", tab.label.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Two spaces: w1 "app" with tabs t1 and t2, and w2 "notes" with t1.
    fn snapshot() -> Snapshot {
        serde_json::from_value(json!({
            "version": "0.7.4",
            "protocol": 16,
            "workspaces": [
                {"workspace_id": "w1", "label": "app", "number": 1, "focused": true,
                 "active_tab_id": "w1:t1", "agent_status": "idle"},
                {"workspace_id": "w2", "label": "", "number": 2, "focused": false,
                 "active_tab_id": "w2:t1", "agent_status": "unknown"},
            ],
            "tabs": [
                {"tab_id": "w1:t1", "workspace_id": "w1", "label": "agents", "number": 1,
                 "focused": true, "pane_count": 2, "agent_status": "idle"},
                {"tab_id": "w1:t2", "workspace_id": "w1", "label": "", "number": 2,
                 "focused": false, "pane_count": 1, "agent_status": "unknown"},
                {"tab_id": "w2:t1", "workspace_id": "w2", "label": "logs", "number": 1,
                 "focused": false, "pane_count": 1, "agent_status": "unknown"},
            ],
            "panes": [
                {"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1",
                 "terminal_id": "t1", "focused": true, "agent_status": "idle"},
                {"pane_id": "w1:p9", "tab_id": "w1:t1", "workspace_id": "w1",
                 "terminal_id": "t9", "focused": false, "agent_status": "unknown"},
            ],
        }))
        .expect("snapshot fixture should deserialize")
    }

    fn labels(destinations: &[Destination]) -> Vec<String> {
        destinations.iter().map(Destination::label).collect()
    }

    #[test]
    fn offers_splitting_here_first_so_enter_enter_is_the_fast_path() {
        let destinations = destinations(&snapshot(), Some("w1:p1"), Some("w1:p9"));
        assert_eq!(
            destinations[0],
            Destination::SplitHere {
                tab_id: "w1:t1".into(),
                target_pane_id: "w1:p1".into(),
                direction: SplitDirection::Right,
            }
        );
        assert_eq!(labels(&destinations)[..2], ["split right", "split down"]);
    }

    #[test]
    fn lists_every_other_tab_and_every_space_and_ends_with_a_new_one() {
        assert_eq!(
            labels(&destinations(&snapshot(), Some("w1:p1"), None)),
            [
                "split right",
                "split down",
                // w1:t1 is missing on purpose: "split here" already covers it.
                "w1  app / w1:t2",
                "w2 / logs",
                "w1  app \u{2192} new tab",
                "w2 \u{2192} new tab",
                "open as a new space",
            ]
        );
    }

    #[test]
    fn falls_back_to_tabs_and_spaces_when_there_is_no_pane_to_split() {
        // Summoned from somewhere herdr could not attribute to a pane.
        let destinations = destinations(&snapshot(), None, None);
        assert!(!destinations
            .iter()
            .any(|d| matches!(d, Destination::SplitHere { .. })));
        assert_eq!(
            labels(&destinations)[0],
            "w1  app / agents",
            "with no current tab, every tab is offered"
        );
    }

    #[test]
    fn never_offers_to_split_the_pickers_own_overlay() {
        let destinations = destinations(&snapshot(), Some("w1:p1"), Some("w1:p1"));
        assert!(!destinations
            .iter()
            .any(|d| matches!(d, Destination::SplitHere { .. })));
    }

    #[test]
    fn splitting_here_targets_the_exact_pane_the_picker_came_from() {
        let destination = Destination::SplitHere {
            tab_id: "w1:t1".into(),
            target_pane_id: "w1:p1".into(),
            direction: SplitDirection::Down,
        };
        assert_eq!(
            placement_for(&destination),
            Some(PaneDestination::Tab {
                tab_id: "w1:t1".into(),
                split: SplitDirection::Down,
                target_pane_id: Some("w1:p1".into()),
            })
        );
    }

    #[test]
    fn another_tab_is_split_wherever_herdr_thinks_best() {
        let destination = Destination::ExistingTab {
            tab_id: "w2:t1".into(),
            label: "w2 / logs".into(),
        };
        assert_eq!(
            placement_for(&destination),
            Some(PaneDestination::Tab {
                tab_id: "w2:t1".into(),
                split: SplitDirection::Right,
                target_pane_id: None,
            })
        );
    }

    #[test]
    fn an_existing_space_gets_a_new_tab_rather_than_a_split() {
        let destination = Destination::ExistingSpace {
            workspace_id: "w2".into(),
            label: "w2 \u{2192} new tab".into(),
        };
        assert_eq!(
            placement_for(&destination),
            Some(PaneDestination::NewTab {
                workspace_id: Some("w2".into()),
            })
        );
    }

    #[test]
    fn a_new_space_needs_no_move_because_that_is_what_herdr_already_did() {
        assert_eq!(placement_for(&Destination::NewSpace), None);
    }
}
