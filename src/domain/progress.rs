//! The steps of opening a branch, as the picker announces them.
//!
//! Choosing a branch can mean a network fetch and a checkout of a whole working tree. That
//! is seconds of work, and a picker that goes blank for it is indistinguishable from one
//! that has crashed. Every step says what it is doing and which branch it is doing it to.
//!
//! Each stage also knows whether the picker can still be abandoned at that point, which is
//! not a question about patience: see [`Stage::interruptible`].

use crate::domain::dest::Destination;

/// One step of opening a chosen branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Working out what the branch is. Nothing has happened yet.
    Starting { branch: String },
    /// Fetching a branch that exists only on the remote. The slow one, and the only one
    /// whose length depends on somebody else's network.
    Fetching { remote: String, branch: String },
    /// Cutting a worktree, which checks out the whole working tree.
    Creating { branch: String },
    /// Opening a checkout that is already on disk.
    Opening { branch: String },
    /// Putting the new pane where the user asked for it.
    Landing { destination: Destination },
}

impl Stage {
    pub fn label(&self) -> String {
        match self {
            Stage::Starting { branch } => format!("opening {branch}"),
            Stage::Fetching { remote, branch } => format!("fetching {remote}/{branch}"),
            Stage::Creating { branch } => format!("creating the worktree for {branch}"),
            Stage::Opening { branch } => format!("opening the checkout for {branch}"),
            Stage::Landing { destination } => match destination {
                Destination::SplitHere { .. } => {
                    "moving the pane beside the one you came from".to_string()
                }
                Destination::ExistingTab { label, .. }
                | Destination::ExistingSpace { label, .. } => {
                    format!("moving the pane into {label}")
                }
                // herdr already put it in a space of its own; there is nothing to move.
                Destination::NewSpace => "focusing the new pane".to_string(),
            },
        }
    }

    /// Whether `Ctrl-C` can still abandon the picker here.
    ///
    /// True only while nothing has been asked of herdr. Once a worktree has been created,
    /// quitting before its pane has been moved would leave a workspace herdr made and
    /// nobody moved — the residue the whole create-then-move design exists to avoid. A
    /// fetch, by contrast, writes only to `refs/remotes` and can be walked away from.
    pub fn interruptible(&self) -> bool {
        matches!(self, Stage::Starting { .. } | Stage::Fetching { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::SplitDirection;

    #[test]
    fn every_stage_says_what_it_is_doing_and_to_what() {
        assert_eq!(
            Stage::Starting {
                branch: "feat/login".into()
            }
            .label(),
            "opening feat/login"
        );
        assert_eq!(
            Stage::Fetching {
                remote: "origin".into(),
                branch: "feat/login".into()
            }
            .label(),
            "fetching origin/feat/login"
        );
        assert_eq!(
            Stage::Creating {
                branch: "feat/login".into()
            }
            .label(),
            "creating the worktree for feat/login"
        );
        assert_eq!(
            Stage::Opening {
                branch: "fix/crash".into()
            }
            .label(),
            "opening the checkout for fix/crash"
        );
    }

    #[test]
    fn landing_reads_as_what_happens_to_the_pane() {
        assert_eq!(
            Stage::Landing {
                destination: Destination::SplitHere {
                    tab_id: "w1:t1".into(),
                    target_pane_id: "w1:p1".into(),
                    direction: SplitDirection::Right,
                }
            }
            .label(),
            "moving the pane beside the one you came from"
        );
        assert_eq!(
            Stage::Landing {
                destination: Destination::ExistingTab {
                    tab_id: "w1:t2".into(),
                    label: "w1  app / logs".into(),
                }
            }
            .label(),
            "moving the pane into w1  app / logs"
        );
        assert_eq!(
            Stage::Landing {
                destination: Destination::NewSpace
            }
            .label(),
            "focusing the new pane",
            "nothing is moved: herdr already put it in a space of its own"
        );
    }

    #[test]
    fn only_the_stages_before_herdr_is_touched_can_be_interrupted() {
        // The rule that matters: quitting after a worktree exists but before its pane has
        // been moved leaves a workspace nobody asked for.
        assert!(Stage::Starting { branch: "b".into() }.interruptible());
        assert!(Stage::Fetching {
            remote: "origin".into(),
            branch: "b".into()
        }
        .interruptible());

        assert!(!Stage::Creating { branch: "b".into() }.interruptible());
        assert!(!Stage::Opening { branch: "b".into() }.interruptible());
        assert!(!Stage::Landing {
            destination: Destination::NewSpace
        }
        .interruptible());
    }
}
