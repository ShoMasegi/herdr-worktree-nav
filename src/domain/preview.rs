//! What a tab will look like once the branch's pane lands in it.
//!
//! The destination step asks the user to choose where a worktree pane goes; this works out
//! the answer for whichever row the cursor is on, so the choice can be seen rather than
//! imagined.
//!
//! The prediction is exact rather than approximate. Verified against herdr 0.7.4's
//! `pane.move`: a destination with no target pane splits the tab's focused pane, the split
//! ratio defaults to 0.5, and the arriving pane takes the second half — the right of a
//! `right` split, the bottom of a `down` one.

use crate::domain::dest::{self, Destination, Landing};
use crate::port::{AgentStatus, LayoutRect, Pane, Snapshot, SplitDirection};

/// One pane in the predicted layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPane {
    /// Where it sits, in the tab's own coordinate space. The caller scales this to fit.
    pub rect: LayoutRect,
    /// The agent's name, when herdr has one for the pane, and the branch on the pane about
    /// to arrive. `None` is a pane herdr tracks no agent in; what that reads as is
    /// [`ui::words`](crate::ui::words)'s to decide.
    pub label: Option<String>,
    /// Empty for the pane about to arrive: it does not have an id yet.
    pub id: String,
    pub status: AgentStatus,
    pub is_new: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// What the tab will look like.
    Layout {
        at: Landing,
        area: LayoutRect,
        panes: Vec<PreviewPane>,
    },
    /// The destination cannot take the pane, and why.
    Blocked { at: Landing, reason: Refusal },
    /// herdr reported no layout for the tab, so there is nothing honest to draw.
    Unavailable,
}

/// Why a destination cannot take the pane.
///
/// One variant, and an enum rather than a sentence, because the sentence is
/// [`ui::words`](crate::ui::words)'s and the next reason will be another variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// herdr will not move a pane into a zoomed tab.
    Zoomed,
}

/// Work out what the destination under the cursor will produce.
pub fn predict(snapshot: &Snapshot, destination: &Destination, branch: &str) -> Preview {
    match destination {
        Destination::SplitHere {
            tab_id,
            target_pane_id,
            direction,
        } => split_into(snapshot, tab_id, Some(target_pane_id), *direction, branch),
        Destination::ExistingTab { tab, .. } => {
            split_into(snapshot, &tab.tab_id, None, SplitDirection::Right, branch)
        }
        Destination::ExistingSpace { space } => {
            fresh_tab(snapshot, Landing::NewTabIn(space.clone()), branch)
        }
        Destination::NewSpace => fresh_tab(snapshot, Landing::NewSpace, branch),
    }
}

fn split_into(
    snapshot: &Snapshot,
    tab_id: &str,
    target_pane_id: Option<&str>,
    direction: SplitDirection,
    branch: &str,
) -> Preview {
    let Some(layout) = snapshot.layouts.iter().find(|l| l.tab_id == tab_id) else {
        return Preview::Unavailable;
    };
    let at = landing_at(snapshot, tab_id);

    if layout.zoomed {
        return Preview::Blocked {
            at,
            reason: Refusal::Zoomed,
        };
    }

    let target = target_pane_id
        .or(layout.focused_pane_id.as_deref())
        .and_then(|id| layout.panes.iter().find(|p| p.pane_id == id))
        .or_else(|| layout.panes.first());
    let Some(target) = target else {
        return Preview::Unavailable;
    };

    let (kept, arriving) = halve(target.rect, direction);
    let panes = layout
        .panes
        .iter()
        .map(|pane| {
            let rect = if pane.pane_id == target.pane_id {
                kept
            } else {
                pane.rect
            };
            existing(snapshot, &pane.pane_id, rect)
        })
        .chain(std::iter::once(PreviewPane {
            rect: arriving,
            label: Some(branch.to_string()),
            id: String::new(),
            status: AgentStatus::Unknown,
            is_new: true,
        }))
        .collect();

    Preview::Layout {
        at,
        area: layout.area,
        panes,
    }
}

/// A destination that makes a tab of its own: one pane, filling it.
fn fresh_tab(snapshot: &Snapshot, at: Landing, branch: &str) -> Preview {
    // Any tab's area will do — herdr gives every tab the same one — and it keeps the
    // proportions of the new tab honest.
    let area = snapshot
        .layouts
        .first()
        .map(|layout| layout.area)
        .unwrap_or(LayoutRect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        });
    Preview::Layout {
        at,
        area,
        panes: vec![PreviewPane {
            rect: area,
            label: Some(branch.to_string()),
            id: String::new(),
            status: AgentStatus::Unknown,
            is_new: true,
        }],
    }
}

fn existing(snapshot: &Snapshot, pane_id: &str, rect: LayoutRect) -> PreviewPane {
    let pane: Option<&Pane> = snapshot.panes.iter().find(|p| p.pane_id == pane_id);
    PreviewPane {
        rect,
        label: pane.and_then(Pane::display_name).map(str::to_string),
        id: pane_id.to_string(),
        status: pane.map(|p| p.agent_status).unwrap_or_default(),
        is_new: false,
    }
}

/// Split a rectangle in two. The arriving pane takes the second half, which is what herdr's
/// `right` and `down` mean.
fn halve(rect: LayoutRect, direction: SplitDirection) -> (LayoutRect, LayoutRect) {
    match direction {
        SplitDirection::Right => {
            let first = rect.width / 2;
            (
                LayoutRect {
                    width: first,
                    ..rect
                },
                LayoutRect {
                    x: rect.x + first,
                    width: rect.width - first,
                    ..rect
                },
            )
        }
        SplitDirection::Down => {
            let first = rect.height / 2;
            (
                LayoutRect {
                    height: first,
                    ..rect
                },
                LayoutRect {
                    y: rect.y + first,
                    height: rect.height - first,
                    ..rect
                },
            )
        }
    }
}

/// The tab a split would land in, named as herdr names it.
fn landing_at(snapshot: &Snapshot, tab_id: &str) -> Landing {
    snapshot
        .tabs
        .iter()
        .find(|tab| tab.tab_id == tab_id)
        .map(|tab| Landing::Tab(dest::tab_name(snapshot, tab)))
        .unwrap_or(Landing::NewSpace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dest::{SpaceName, TabName};
    use serde_json::json;

    /// `w1` has a tab split left/right; `w2` has a single-pane tab that is zoomed.
    fn snapshot() -> Snapshot {
        serde_json::from_value(json!({
            "version": "0.7.4",
            "protocol": 16,
            "workspaces": [
                {"workspace_id": "w1", "label": "app", "number": 1, "focused": true,
                 "active_tab_id": "w1:t1", "agent_status": "idle"},
                {"workspace_id": "w2", "label": "notes", "number": 2, "focused": false,
                 "active_tab_id": "w2:t1", "agent_status": "unknown"},
            ],
            "tabs": [
                {"tab_id": "w1:t1", "workspace_id": "w1", "label": "agents", "number": 1,
                 "focused": true, "pane_count": 2, "agent_status": "idle"},
                {"tab_id": "w2:t1", "workspace_id": "w2", "label": "logs", "number": 1,
                 "focused": false, "pane_count": 1, "agent_status": "unknown"},
            ],
            "panes": [
                {"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1",
                 "terminal_id": "t1", "focused": true, "agent": "claude",
                 "agent_status": "working"},
                {"pane_id": "w1:p2", "tab_id": "w1:t1", "workspace_id": "w1",
                 "terminal_id": "t2", "focused": false, "agent_status": "unknown"},
                {"pane_id": "w2:p1", "tab_id": "w2:t1", "workspace_id": "w2",
                 "terminal_id": "t3", "focused": true, "agent_status": "unknown"},
            ],
            "layouts": [
                {"tab_id": "w1:t1", "workspace_id": "w1", "zoomed": false,
                 "area": {"x": 0, "y": 0, "width": 100, "height": 40},
                 "focused_pane_id": "w1:p1",
                 "panes": [
                     {"pane_id": "w1:p1", "focused": true,
                      "rect": {"x": 0, "y": 0, "width": 50, "height": 40}},
                     {"pane_id": "w1:p2", "focused": false,
                      "rect": {"x": 50, "y": 0, "width": 50, "height": 40}}
                 ]},
                {"tab_id": "w2:t1", "workspace_id": "w2", "zoomed": true,
                 "area": {"x": 0, "y": 0, "width": 100, "height": 40},
                 "focused_pane_id": "w2:p1",
                 "panes": [{"pane_id": "w2:p1", "focused": true,
                            "rect": {"x": 0, "y": 0, "width": 100, "height": 40}}]}
            ],
        }))
        .expect("snapshot fixture should deserialize")
    }

    /// A tab as herdr would have handed it over, spelled out rather than read off the
    /// snapshot: these tests are about what a destination predicts, not about how a
    /// destination is built.
    fn tab_name(
        workspace: &str,
        space: Option<&str>,
        tab_id: &str,
        label: Option<&str>,
    ) -> TabName {
        TabName {
            space: SpaceName {
                workspace_id: workspace.into(),
                label: space.map(str::to_string),
            },
            tab_id: tab_id.into(),
            label: label.map(str::to_string),
        }
    }

    fn layout_of(preview: Preview) -> (Landing, Vec<PreviewPane>) {
        match preview {
            Preview::Layout { at, panes, .. } => (at, panes),
            other => panic!("expected a layout, got {other:?}"),
        }
    }

    /// The tab a landing names, or a panic saying what it named instead.
    fn tab_of(at: &Landing) -> &TabName {
        match at {
            Landing::Tab(tab) => tab,
            other => panic!("expected a tab, got {other:?}"),
        }
    }

    /// A pane that is already open, by the id herdr knows it as. The arriving one has no id
    /// yet, so [`arriving`] is how that one is asked for.
    fn find<'a>(panes: &'a [PreviewPane], id: &str) -> &'a PreviewPane {
        panes
            .iter()
            .find(|pane| pane.id == id)
            .unwrap_or_else(|| panic!("no pane with id {id}"))
    }

    fn arriving(panes: &[PreviewPane]) -> &PreviewPane {
        panes
            .iter()
            .find(|pane| pane.is_new)
            .expect("a preview always carries the pane about to arrive")
    }

    #[test]
    fn splitting_a_pane_halves_it_and_puts_the_branch_in_the_second_half() {
        let destination = Destination::SplitHere {
            tab_id: "w1:t1".into(),
            target_pane_id: "w1:p1".into(),
            direction: SplitDirection::Right,
        };
        let (at, panes) = layout_of(predict(&snapshot(), &destination, "feat/login"));
        assert_eq!(tab_of(&at).tab_id, "w1:t1");
        assert_eq!(panes.len(), 3);

        let claude = find(&panes, "w1:p1");
        assert_eq!(claude.label.as_deref(), Some("claude"));
        assert_eq!(
            claude.rect,
            LayoutRect {
                x: 0,
                y: 0,
                width: 25,
                height: 40
            }
        );
        let arriving = arriving(&panes);
        assert_eq!(arriving.label.as_deref(), Some("feat/login"));
        assert_eq!(
            arriving.rect,
            LayoutRect {
                x: 25,
                y: 0,
                width: 25,
                height: 40
            }
        );
        assert!(arriving.id.is_empty(), "it does not have an id yet");

        let untracked = find(&panes, "w1:p2");
        assert_eq!(
            untracked.label, None,
            "herdr tracks no agent here, and what that reads as is not this layer's call"
        );
        assert_eq!(
            untracked.rect,
            LayoutRect {
                x: 50,
                y: 0,
                width: 50,
                height: 40
            }
        );
    }

    #[test]
    fn splitting_downwards_halves_the_other_way() {
        let destination = Destination::SplitHere {
            tab_id: "w1:t1".into(),
            target_pane_id: "w1:p1".into(),
            direction: SplitDirection::Down,
        };
        let (_, panes) = layout_of(predict(&snapshot(), &destination, "feat/login"));
        assert_eq!(
            find(&panes, "w1:p1").rect,
            LayoutRect {
                x: 0,
                y: 0,
                width: 50,
                height: 20
            }
        );
        assert_eq!(
            arriving(&panes).rect,
            LayoutRect {
                x: 0,
                y: 20,
                width: 50,
                height: 20
            }
        );
    }

    #[test]
    fn a_tab_with_no_target_named_splits_the_pane_it_has_focused() {
        let destination = Destination::ExistingTab {
            tab: tab_name("w1", Some("app"), "w1:t1", Some("agents")),
            zoomed: false,
        };
        let (_, panes) = layout_of(predict(&snapshot(), &destination, "feat/login"));
        assert_eq!(
            find(&panes, "w1:p1").rect.width,
            25,
            "w1:p1 is focused, so it is the one that gives up half"
        );
        assert_eq!(find(&panes, "w1:p2").rect.width, 50);
    }

    #[test]
    fn a_zoomed_tab_is_reported_as_blocked_rather_than_drawn() {
        // herdr answers a move into a zoomed tab with success and does nothing, so the only
        // honest preview is to say it will not work.
        let destination = Destination::ExistingTab {
            tab: tab_name("w2", Some("notes"), "w2:t1", Some("logs")),
            zoomed: true,
        };
        let Preview::Blocked { at, reason } = predict(&snapshot(), &destination, "feat/login")
        else {
            panic!("a zoomed tab should be blocked");
        };
        assert_eq!(tab_of(&at).tab_id, "w2:t1");
        assert_eq!(reason, Refusal::Zoomed);
    }

    #[test]
    fn a_destination_that_makes_its_own_tab_previews_as_a_single_pane() {
        let space = SpaceName {
            workspace_id: "w2".into(),
            label: Some("notes".into()),
        };
        for (destination, expected) in [
            (
                Destination::ExistingSpace {
                    space: space.clone(),
                },
                Landing::NewTabIn(space),
            ),
            (Destination::NewSpace, Landing::NewSpace),
        ] {
            let (at, panes) = layout_of(predict(&snapshot(), &destination, "feat/login"));
            assert_eq!(at, expected);
            assert_eq!(panes.len(), 1);
            assert!(panes[0].is_new);
            assert_eq!(
                panes[0].rect,
                LayoutRect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 40
                }
            );
        }
    }

    #[test]
    fn a_tab_herdr_reported_no_layout_for_is_left_undrawn() {
        let destination = Destination::ExistingTab {
            tab: tab_name("w9", None, "w9:t9", None),
            zoomed: false,
        };
        assert_eq!(
            predict(&snapshot(), &destination, "feat/login"),
            Preview::Unavailable
        );
    }

    #[test]
    fn an_odd_width_gives_the_extra_column_to_the_arriving_pane() {
        let mut snapshot = snapshot();
        snapshot.layouts[0].panes[0].rect.width = 51;
        let destination = Destination::SplitHere {
            tab_id: "w1:t1".into(),
            target_pane_id: "w1:p1".into(),
            direction: SplitDirection::Right,
        };
        let (_, panes) = layout_of(predict(&snapshot, &destination, "feat/login"));
        assert_eq!(find(&panes, "w1:p1").rect.width, 25);
        assert_eq!(arriving(&panes).rect.width, 26);
    }
}
