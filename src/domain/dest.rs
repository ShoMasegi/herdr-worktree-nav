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

/// A workspace as herdr has it: the id it is addressed by, and the name it was given.
///
/// Both halves, because both are shown: the id is what the user sees in herdr's own
/// navigator, and the name is the only thing that says which project it is. Which of them a
/// row reads as is [`ui::words`](crate::ui::words)'s to decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceName {
    pub workspace_id: String,
    /// `None` for a workspace herdr was never given a name for.
    pub label: Option<String>,
}

/// A tab as herdr has it, and the space it sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabName {
    pub space: SpaceName,
    pub tab_id: String,
    /// `None` for a tab herdr was never given a name for.
    pub label: Option<String>,
}

/// Where the pane would land, for the preview and for the row that offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landing {
    /// A tab that is already open.
    Tab(TabName),
    /// A new tab in a space that is already open.
    NewTabIn(SpaceName),
    /// The space herdr makes for the worktree, left where it is.
    NewSpace,
}

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
    ExistingTab {
        tab: TabName,
        /// herdr will not move a pane into a zoomed tab. Carried rather than filtered out:
        /// the tab is on screen, and "it is not in the list" is a worse answer than "it is,
        /// and here is why you cannot use it".
        zoomed: bool,
    },
    /// Add a tab to an existing space.
    ExistingSpace { space: SpaceName },
    /// Leave the workspace herdr made — the built-in behaviour.
    NewSpace,
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
        Destination::ExistingTab { tab, .. } => Some(PaneDestination::Tab {
            tab_id: tab.tab_id.clone(),
            split: SplitDirection::Right,
            // No target: herdr splits the tab's active pane.
            target_pane_id: None,
        }),
        Destination::ExistingSpace { space } => Some(PaneDestination::NewTab {
            workspace_id: Some(space.workspace_id.clone()),
        }),
        Destination::NewSpace => None,
    }
}

/// Build the destination list for a picker summoned from `from_pane_id`.
///
/// The picker itself is never a candidate: it runs as a popup, which is not a pane.
pub fn destinations(snapshot: &Snapshot, from_pane_id: Option<&str>) -> Vec<Destination> {
    let mut destinations = Vec::new();

    let origin = from_pane_id.and_then(|id| snapshot.panes.iter().find(|pane| pane.pane_id == id));

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
        let zoomed = snapshot
            .layouts
            .iter()
            .any(|layout| layout.tab_id == tab.tab_id && layout.zoomed);
        destinations.push(Destination::ExistingTab {
            tab: tab_name(snapshot, tab),
            zoomed,
        });
    }

    for workspace in &snapshot.workspaces {
        destinations.push(Destination::ExistingSpace {
            space: space_name(workspace),
        });
    }

    destinations.push(Destination::NewSpace);
    destinations
}

pub(crate) fn space_name(workspace: &Workspace) -> SpaceName {
    let label = workspace.label.trim();
    SpaceName {
        workspace_id: workspace.workspace_id.clone(),
        label: (!label.is_empty()).then(|| label.to_string()),
    }
}

pub(crate) fn tab_name(snapshot: &Snapshot, tab: &Tab) -> TabName {
    let space = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == tab.workspace_id)
        .map(space_name)
        .unwrap_or(SpaceName {
            workspace_id: tab.workspace_id.clone(),
            label: None,
        });
    let label = tab.label.trim();
    TabName {
        space,
        tab_id: tab.tab_id.clone(),
        label: (!label.is_empty()).then(|| label.to_string()),
    }
}

/// Names spelled out by hand, for tests about what is done with a destination rather than
/// about how one is built.
///
/// Here rather than beside any one of them because a destination is named in the row that
/// offers it, in the preview of where it leads and in the step that lands a pane there, and
/// those tests live in three modules.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::{SpaceName, TabName};

    pub(crate) fn space_name(workspace_id: &str, label: &str) -> SpaceName {
        SpaceName {
            workspace_id: workspace_id.into(),
            label: (!label.is_empty()).then(|| label.to_string()),
        }
    }

    pub(crate) fn tab_name(workspace_id: &str, space: &str, tab_id: &str, label: &str) -> TabName {
        TabName {
            space: space_name(workspace_id, space),
            tab_id: tab_id.into(),
            label: (!label.is_empty()).then(|| label.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    /// The destinations by kind and by the id each one addresses, which is what the list
    /// is: the wording every row reads as is [`ui::words`](crate::ui::words)'s, and asked
    /// for there.
    fn shape(destinations: &[Destination]) -> Vec<String> {
        destinations
            .iter()
            .map(|destination| match destination {
                Destination::SplitHere { direction, .. } => {
                    format!("split {}", direction.as_str())
                }
                Destination::ExistingTab { tab, zoomed } => {
                    format!("tab {}{}", tab.tab_id, if *zoomed { " zoomed" } else { "" })
                }
                Destination::ExistingSpace { space } => format!("space {}", space.workspace_id),
                Destination::NewSpace => "new space".to_string(),
            })
            .collect()
    }

    #[test]
    fn offers_splitting_here_first_so_enter_enter_is_the_fast_path() {
        let destinations = destinations(&snapshot(), Some("w1:p1"));
        assert_eq!(
            destinations[0],
            Destination::SplitHere {
                tab_id: "w1:t1".into(),
                target_pane_id: "w1:p1".into(),
                direction: SplitDirection::Right,
            }
        );
        assert_eq!(shape(&destinations)[..2], ["split right", "split down"]);
    }

    #[test]
    fn lists_every_other_tab_and_every_space_and_ends_with_a_new_one() {
        assert_eq!(
            shape(&destinations(&snapshot(), Some("w1:p1"))),
            [
                "split right",
                "split down",
                // w1:t1 is missing on purpose: "split here" already covers it.
                "tab w1:t2",
                "tab w2:t1",
                "space w1",
                "space w2",
                "new space",
            ]
        );
    }

    #[test]
    fn a_tab_carries_the_name_of_the_space_it_is_in() {
        let destinations = destinations(&snapshot(), Some("w1:p1"));
        let Some(Destination::ExistingTab { tab, .. }) = destinations
            .iter()
            .find(|d| matches!(d, Destination::ExistingTab { tab, .. } if tab.tab_id == "w2:t1"))
        else {
            panic!(
                "expected w2:t1 to be offered, got {:?}",
                shape(&destinations)
            );
        };
        assert_eq!(tab.label.as_deref(), Some("logs"));
        assert_eq!(tab.space.workspace_id, "w2");
        assert_eq!(
            tab.space.label, None,
            "a workspace herdr gave no name is carried as having none, not as an empty one"
        );
    }

    #[test]
    fn a_tab_with_no_name_of_its_own_carries_none() {
        let destinations = destinations(&snapshot(), Some("w1:p1"));
        let Some(Destination::ExistingTab { tab, .. }) = destinations
            .iter()
            .find(|d| matches!(d, Destination::ExistingTab { tab, .. } if tab.tab_id == "w1:t2"))
        else {
            panic!(
                "expected w1:t2 to be offered, got {:?}",
                shape(&destinations)
            );
        };
        assert_eq!(tab.label, None);
        assert_eq!(tab.space.label.as_deref(), Some("app"));
    }

    #[test]
    fn a_zoomed_tab_is_listed_but_marked() {
        let mut snapshot = snapshot();
        snapshot.layouts = serde_json::from_value(json!([
            {"tab_id": "w2:t1", "workspace_id": "w2", "zoomed": true,
             "area": {"x": 0, "y": 0, "width": 80, "height": 24},
             "focused_pane_id": "w2:p1", "panes": []}
        ]))
        .unwrap();
        let shape = shape(&destinations(&snapshot, Some("w1:p1")));
        assert!(
            shape.contains(&"tab w2:t1 zoomed".to_string()),
            "got {shape:?}"
        );
    }

    #[test]
    fn falls_back_to_tabs_and_spaces_when_there_is_no_pane_to_split() {
        let destinations = destinations(&snapshot(), None);
        assert!(!destinations
            .iter()
            .any(|d| matches!(d, Destination::SplitHere { .. })));
        assert_eq!(
            shape(&destinations)[0],
            "tab w1:t1",
            "with no current tab, every tab is offered"
        );
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
            tab: TabName {
                space: SpaceName {
                    workspace_id: "w2".into(),
                    label: None,
                },
                tab_id: "w2:t1".into(),
                label: Some("logs".into()),
            },
            zoomed: false,
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
            space: SpaceName {
                workspace_id: "w2".into(),
                label: None,
            },
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
