//! Every sentence the picker puts under the list: what the row under the cursor is, what
//! the repository above it holds, and what the chosen destination would do.

use crate::domain::dest::Destination;
use crate::domain::preview::{self, Preview};
use crate::domain::resolve::{BranchState, Chosen};
use crate::ui::branches::*;

impl BranchesState {
    /// The breadcrumb under the repository list: the full path of the row under the cursor.
    pub fn repo_detail(&self) -> String {
        let Some(row) = self.repo_visible.get(self.repo_cursor) else {
            return String::new();
        };
        let repo = &self.repos[*row];
        format!("{} \u{b7} {}", repo.display_name, repo.repo_root)
    }

    /// The heading over the branch list: which repository these branches belong to, and
    /// where it is on disk.
    ///
    /// The same words the repository list put under the cursor, so choosing one moves the
    /// line you were reading from the bottom of the screen to the top. With one repository
    /// open there is no repository step at all, and this is the only place it is named.
    pub fn repo_heading(&self) -> String {
        let repo = self.repo();
        format!("{} \u{b7} {}", repo.display_name, repo.repo_root)
    }

    /// The breadcrumb under the list: where this branch is, and what picking it will do.
    ///
    /// It does not name the repository. Every row in the list belongs to the same one, so
    /// repeating it here would spend width on a constant; the heading above the list says it
    /// once.
    pub fn detail(&self) -> String {
        let Some(entry) = self.selected() else {
            return String::new();
        };
        let mut parts = vec![entry.name.clone()];
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
        let (Some(destination), Some(chosen)) = (
            self.destinations.get(self.destination_cursor),
            self.chosen.as_ref(),
        ) else {
            return Preview::Unavailable;
        };
        preview::predict(&self.snapshot, destination, chosen.name())
    }

    /// Whether the destination under the cursor can actually take the pane.
    pub(super) fn destination_is_blocked(&self) -> Option<String> {
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
            .map(Chosen::name)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::branches::fixtures::*;
    use ratatui::crossterm::event::KeyCode;

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
        search(&mut state, "chore");
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
        search(&mut state, "chore");
        state.handle_key(key(KeyCode::Enter));
        let Preview::Layout { panes, .. } = state.preview() else {
            panic!("expected a layout, got {:?}", state.preview());
        };
        assert_eq!(panes.len(), 2);
        assert!(panes.iter().any(|p| p.is_new && p.label == "chore/deps"));
    }
}
