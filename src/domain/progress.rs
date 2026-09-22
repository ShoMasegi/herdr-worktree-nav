//! The steps of opening a branch, as the picker announces them.
//!
//! Choosing a branch can mean a network fetch and a checkout of a whole working tree. That
//! is seconds of work, and a picker that goes blank for it is indistinguishable from one
//! that has crashed. Every step says what it is doing and which branch it is doing it to.

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

    #[test]
    fn only_the_stages_before_herdr_is_touched_can_be_interrupted() {
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
