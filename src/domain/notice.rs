//! What the picker has to tell the user, and how long each thing stays true.
//!
//! Two kinds of sentence reach the prompt line, and they differ in lifetime.
//!
//! A **condition** is true until something measures it again: a repository whose refs git
//! would not read, a list that is behind. This module is that half, and nothing in it is
//! stored — [`conditions`] is handed what the frame knows and builds the list afresh. That
//! is what makes a condition retire correctly: a frame where it no longer holds does not
//! produce it, so nothing has to remember to take it back.
//!
//! An **event** is true at a moment: a removal was refused, a key did not apply here. That
//! half is pushed rather than derived, and retires on the next act that could have answered
//! it. `ui::panes` owns it.
//!
//! Getting the two the wrong way round is what issues #64 and #35 are about, and
//! `docs/en/error-handling.md` is where the distinction is written down.

use crate::domain::model::{Refs, Tree};

/// One thing the picker has to say about the list it is showing.
///
/// A value rather than a sentence: which of these hold is derived every frame, and what
/// each one reads as on a prompt line that may have no room for it is
/// [`ui::words`](crate::ui::words)'s to decide. There is deliberately no severity here yet.
/// Both conditions produced today are about one repository, so an ordering field would be a
/// guess; the order is the order they are gathered in, and [`conditions`] says what that
/// order means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// The reading that built the list failed, so the rows may be behind. Carries the words
    /// of whatever refused, which the picker has no better account of.
    Stale(String),
    /// git would not read this repository's refs, so every track marker in it is missing.
    RefsUnreadable { repo: String, words: String },
    /// What could not be asked of `gh` in this sweep, already counted by
    /// [`app::settled`](crate::app::settled) because which repositories were asked is not
    /// something the tree knows.
    SweepTrouble(String),
}

/// Everything that is wrong right now, worst first.
///
/// git not reading a repository's refs comes before what `gh` said, and that is the order
/// they are gathered in. The first is about the track markers on every row of the
/// repository and is true sweep or no sweep; `gh` is asked only during a sweep, and only
/// about the half git could not decide. A condition that is about the whole session rather
/// than one repository belongs in front of both — issues #33 and #56 are the two that will
/// want that, and this is the function they add a line to.
///
/// One state that draws no marker deliberately has no line here: a single ref whose
/// `:track` field git printed and the adapter could not read. Why it does not is written
/// where the state is, on [`Track::Unreadable`](crate::port::Track::Unreadable).
///
/// `stale` and `sweep_trouble` are passed in rather than read off the tree. The first is
/// about the reading that built it, which a tree cannot report about itself; the second is
/// already one sentence for however many repositories `gh` could not be asked about, which
/// [`app::settled`](crate::app::settled) builds because which repositories were asked is
/// not something the tree
/// knows.
pub fn conditions(tree: &Tree, stale: Option<&str>, sweep_trouble: Option<&str>) -> Vec<Condition> {
    // In front of the rest: the others are about what a row is missing, and this one is
    // about whether the rows are the right rows at all.
    let mut conditions: Vec<Condition> = stale
        .map(|words| Condition::Stale(words.to_string()))
        .into_iter()
        .collect();
    conditions.extend(tree.repos.iter().filter_map(|repo| match &repo.refs {
        Refs::Read => None,
        Refs::Unreadable(words) => Some(Condition::RefsUnreadable {
            repo: repo.display_name.clone(),
            words: words.clone(),
        }),
    }));
    conditions.extend(sweep_trouble.map(|words| Condition::SweepTrouble(words.to_string())));
    conditions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{RepoNode, Tree};

    fn repo(name: &str, refs: Refs) -> RepoNode {
        RepoNode {
            repo_key: format!("/{name}/.git"),
            repo_root: format!("/{name}"),
            display_name: name.to_string(),
            worktrees: Vec::new(),
            refs,
        }
    }

    fn tree(repos: Vec<RepoNode>) -> Tree {
        Tree {
            repos,
            ..Default::default()
        }
    }

    #[test]
    fn a_tree_git_answered_for_with_no_sweep_trouble_has_nothing_to_say() {
        let tree = tree(vec![repo("me/app", Refs::Read)]);
        assert_eq!(conditions(&tree, None, None), Vec::new());
    }

    #[test]
    fn every_repository_whose_refs_were_not_read_is_its_own_condition() {
        let tree = tree(vec![
            repo("me/app", Refs::Unreadable("fatal: bad ref".into())),
            repo("me/site", Refs::Read),
            repo(
                "me/docs",
                Refs::Unreadable("fatal: index file corrupt".into()),
            ),
        ]);
        assert_eq!(
            conditions(&tree, None, None),
            vec![
                Condition::RefsUnreadable {
                    repo: "me/app".into(),
                    words: "fatal: bad ref".into(),
                },
                Condition::RefsUnreadable {
                    repo: "me/docs".into(),
                    words: "fatal: index file corrupt".into(),
                },
            ]
        );
    }

    #[test]
    fn what_gh_said_is_gathered_behind_what_git_said() {
        let tree = tree(vec![repo(
            "me/app",
            Refs::Unreadable("fatal: bad ref".into()),
        )]);
        assert_eq!(
            conditions(&tree, None, Some("me/app: gh could not be run")),
            vec![
                Condition::RefsUnreadable {
                    repo: "me/app".into(),
                    words: "fatal: bad ref".into(),
                },
                Condition::SweepTrouble("me/app: gh could not be run".into()),
            ],
            "the track markers are missing sweep or no sweep; gh is only asked during one"
        );
    }

    #[test]
    fn what_gh_said_stands_alone_once_git_has_answered() {
        let tree = tree(vec![repo("me/app", Refs::Read)]);
        assert_eq!(
            conditions(&tree, None, Some("me/app: gh could not be run")),
            vec![Condition::SweepTrouble(
                "me/app: gh could not be run".into()
            )]
        );
    }

    #[test]
    fn a_list_that_may_be_behind_is_said_before_what_any_one_row_is_missing() {
        let tree = tree(vec![repo(
            "me/app",
            Refs::Unreadable("fatal: bad ref".into()),
        )]);
        assert_eq!(
            conditions(&tree, Some("herdr did not answer"), None)[0],
            Condition::Stale("herdr did not answer".into()),
            "whether these are the right rows at all comes before what one of them lacks"
        );
    }
}
