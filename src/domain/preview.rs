//! What a tab will look like once the branch's pane lands in it.
//!
//! The destination step asks the user to choose where a worktree pane goes; this works out
//! the answer for whichever row the cursor is on, so the choice can be seen rather than
//! imagined. Pure: it takes herdr's current layouts and returns the predicted one.
//!
//! The prediction is exact rather than approximate. Verified against herdr 0.7.4's
//! `pane.move`: a destination with no target pane splits the tab's focused pane, the split
//! ratio defaults to 0.5, and the arriving pane takes the second half — the right of a
//! `right` split, the bottom of a `down` one.

use crate::domain::dest::Destination;
use crate::port::{AgentStatus, Layout, LayoutRect, Pane, Snapshot, SplitDirection};

/// One pane in the predicted layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPane {
    /// Where it sits, in the tab's own coordinate space. The caller scales this to fit.
    pub rect: LayoutRect,
    /// The agent's name, or `shell`, or the branch for the pane about to arrive.
    pub label: String,
    /// Empty for the pane about to arrive: it does not have an id yet.
    pub id: String,
    pub status: AgentStatus,
    pub is_new: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// What the tab will look like.
    Layout {
        caption: String,
        area: LayoutRect,
        panes: Vec<PreviewPane>,
    },
    /// The destination cannot take the pane, and why.
    Blocked { caption: String, reason: String },
    /// herdr reported no layout for the tab, so there is nothing honest to draw.
    Unavailable,
}

/// Shown for a pane herdr is not tracking an agent in.
const UNNAMED_PANE: &str = "shell";

/// Work out what the destination under the cursor will produce.
pub fn predict(snapshot: &Snapshot, destination: &Destination, branch: &str) -> Preview {
    match destination {
        Destination::SplitHere {
            tab_id,
            target_pane_id,
            direction,
        } => split_into(snapshot, tab_id, Some(target_pane_id), *direction, branch),
        Destination::ExistingTab { tab_id, .. } => {
            // No target pane: herdr splits whichever pane that tab has focused.
            split_into(snapshot, tab_id, None, SplitDirection::Right, branch)
        }
        Destination::ExistingSpace {
            workspace_id,
            label,
        } => fresh_tab(snapshot, Some(workspace_id), label, branch),
        Destination::NewSpace => fresh_tab(snapshot, None, "a space of its own", branch),
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
    let caption = caption_for(snapshot, layout);

    if layout.zoomed {
        return Preview::Blocked {
            caption,
            reason: "this tab is zoomed, and herdr will not move a pane into a zoomed tab. \
                     Unzoom it first, or pick somewhere else."
                .to_string(),
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
            label: branch.to_string(),
            id: String::new(),
            status: AgentStatus::Unknown,
            is_new: true,
        }))
        .collect();

    Preview::Layout {
        caption,
        area: layout.area,
        panes,
    }
}

/// A destination that makes a tab of its own: one pane, filling it.
fn fresh_tab(
    snapshot: &Snapshot,
    workspace_id: Option<&str>,
    label: &str,
    branch: &str,
) -> Preview {
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
    let caption = match workspace_id {
        Some(_) => format!("{label} \u{2014} a new tab"),
        None => format!("{label} \u{2014} a new space"),
    };
    Preview::Layout {
        caption,
        area,
        panes: vec![PreviewPane {
            rect: area,
            label: branch.to_string(),
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
        label: pane
            .and_then(Pane::display_name)
            .unwrap_or(UNNAMED_PANE)
            .to_string(),
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

/// `w5  harken / android`, the same wording the destination list uses.
fn caption_for(snapshot: &Snapshot, layout: &Layout) -> String {
    snapshot
        .tabs
        .iter()
        .find(|tab| tab.tab_id == layout.tab_id)
        .map(|tab| crate::domain::dest::tab_label(snapshot, tab))
        .unwrap_or_else(|| layout.tab_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn layout_of(preview: Preview) -> (String, Vec<PreviewPane>) {
        match preview {
            Preview::Layout { caption, panes, .. } => (caption, panes),
            other => panic!("expected a layout, got {other:?}"),
        }
    }

    fn find<'a>(panes: &'a [PreviewPane], label: &str) -> &'a PreviewPane {
        panes
            .iter()
            .find(|p| p.label == label)
            .unwrap_or_else(|| panic!("no pane labelled {label}"))
    }

    #[test]
    fn splitting_a_pane_halves_it_and_puts_the_branch_in_the_second_half() {
        let destination = Destination::SplitHere {
            tab_id: "w1:t1".into(),
            target_pane_id: "w1:p1".into(),
            direction: SplitDirection::Right,
        };
        let (caption, panes) = layout_of(predict(&snapshot(), &destination, "feat/login"));
        assert_eq!(caption, "w1  app / agents");
        assert_eq!(panes.len(), 3);

        // The target keeps the left half.
        let claude = find(&panes, "claude");
        assert_eq!(
            claude.rect,
            LayoutRect {
                x: 0,
                y: 0,
                width: 25,
                height: 40
            }
        );
        // The branch takes the right half of what the target had.
        let arriving = find(&panes, "feat/login");
        assert_eq!(
            arriving.rect,
            LayoutRect {
                x: 25,
                y: 0,
                width: 25,
                height: 40
            }
        );
        assert!(arriving.is_new);
        assert!(arriving.id.is_empty(), "it does not have an id yet");
        // The pane that was not split is untouched.
        assert_eq!(
            find(&panes, "shell").rect,
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
            find(&panes, "claude").rect,
            LayoutRect {
                x: 0,
                y: 0,
                width: 50,
                height: 20
            }
        );
        assert_eq!(
            find(&panes, "feat/login").rect,
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
        // Mirrors herdr: pane.move with no target_pane_id uses the tab's focused pane.
        let destination = Destination::ExistingTab {
            tab_id: "w1:t1".into(),
            label: "w1  app / agents".into(),
        };
        let (_, panes) = layout_of(predict(&snapshot(), &destination, "feat/login"));
        assert_eq!(
            find(&panes, "claude").rect.width,
            25,
            "w1:p1 is focused, so it is the one that gives up half"
        );
        assert_eq!(find(&panes, "shell").rect.width, 50);
    }

    #[test]
    fn a_zoomed_tab_is_reported_as_blocked_rather_than_drawn() {
        // herdr answers a move into a zoomed tab with success and does nothing, so the only
        // honest preview is to say it will not work.
        let destination = Destination::ExistingTab {
            tab_id: "w2:t1".into(),
            label: "w2  notes / logs".into(),
        };
        let Preview::Blocked { caption, reason } = predict(&snapshot(), &destination, "feat/login")
        else {
            panic!("a zoomed tab should be blocked");
        };
        assert_eq!(caption, "w2  notes / logs");
        assert!(reason.contains("zoomed"), "got {reason}");
    }

    #[test]
    fn a_destination_that_makes_its_own_tab_previews_as_a_single_pane() {
        for (destination, expected) in [
            (
                Destination::ExistingSpace {
                    workspace_id: "w2".into(),
                    label: "w2  notes".into(),
                },
                "w2  notes — a new tab",
            ),
            (Destination::NewSpace, "a space of its own — a new space"),
        ] {
            let (caption, panes) = layout_of(predict(&snapshot(), &destination, "feat/login"));
            assert_eq!(caption, expected);
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
            tab_id: "w9:t9".into(),
            label: "gone".into(),
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
        assert_eq!(find(&panes, "claude").rect.width, 25);
        assert_eq!(find(&panes, "feat/login").rect.width, 26);
    }
}
