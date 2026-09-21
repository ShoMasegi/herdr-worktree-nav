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
//! it. `ui::state` owns it.
//!
//! Getting the two the wrong way round is what issues #64 and #35 are about, and
//! `docs/en/error-handling.md` is where the distinction is written down.

use crate::domain::model::{Refs, Tree};

/// One thing the picker has to say, flattened to the single line it may have to fit on.
///
/// A struct rather than a bare `String` because the list is what the whole mechanism hangs
/// on: [`summarize`] shows the first and counts the rest, and what is counted has to be
/// readable somewhere. There is deliberately no severity here yet. Both conditions produced
/// today are about one repository, so an ordering field would be a guess; the order is the
/// order they are gathered in, and [`conditions`] says what that order means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
}

impl Notice {
    pub fn new(text: impl Into<String>) -> Self {
        Notice { text: text.into() }
    }
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
/// `sweep_trouble` is already one sentence for however many repositories `gh` could not be
/// asked about; `app::settled` builds it, because which repositories were asked is not
/// something the tree knows.
pub fn conditions(tree: &Tree, sweep_trouble: Option<&str>) -> Vec<Notice> {
    let mut notices: Vec<Notice> = tree
        .repos
        .iter()
        .filter_map(|repo| match &repo.refs {
            Refs::Read => None,
            Refs::Unreadable(words) => Some(Notice::new(format!(
                "{}: refs unreadable: {words}",
                repo.display_name
            ))),
        })
        .collect();
    notices.extend(sweep_trouble.map(Notice::new));
    notices
}

/// The one-line form: the first sentence, and a count of what it is standing in for.
///
/// One line can hold one of these, so the rest are counted rather than dropped — a line
/// that showed the first and said nothing about the others would read as the whole story.
/// The count is what the reader follows to the unabridged list.
pub fn summarize(notices: &[Notice]) -> Option<String> {
    let first = notices.first()?;
    Some(match notices.len() - 1 {
        0 => first.text.clone(),
        more => format!("{} (+{more} more)", first.text),
    })
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
        assert_eq!(conditions(&tree, None), Vec::new());
        assert_eq!(summarize(&conditions(&tree, None)), None);
    }

    #[test]
    fn every_repository_whose_refs_were_not_read_is_its_own_notice() {
        let tree = tree(vec![
            repo("me/app", Refs::Unreadable("fatal: bad ref".into())),
            repo("me/site", Refs::Read),
            repo(
                "me/docs",
                Refs::Unreadable("fatal: index file corrupt".into()),
            ),
        ]);
        let notices = conditions(&tree, None);
        assert_eq!(
            notices,
            vec![
                Notice::new("me/app: refs unreadable: fatal: bad ref"),
                Notice::new("me/docs: refs unreadable: fatal: index file corrupt"),
            ]
        );
    }

    #[test]
    fn what_gh_said_is_gathered_behind_what_git_said() {
        let tree = tree(vec![repo(
            "me/app",
            Refs::Unreadable("fatal: bad ref".into()),
        )]);
        let notices = conditions(&tree, Some("me/app: gh could not be run"));
        assert_eq!(
            notices.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
            vec![
                "me/app: refs unreadable: fatal: bad ref",
                "me/app: gh could not be run",
            ],
            "the track markers are missing sweep or no sweep; gh is only asked during one"
        );
    }

    #[test]
    fn what_gh_said_stands_alone_once_git_has_answered() {
        let tree = tree(vec![repo("me/app", Refs::Read)]);
        assert_eq!(
            summarize(&conditions(&tree, Some("me/app: gh could not be run"))).as_deref(),
            Some("me/app: gh could not be run")
        );
    }

    #[test]
    fn a_line_that_can_hold_one_says_how_many_it_is_standing_in_for() {
        let tree = tree(vec![
            repo("me/app", Refs::Unreadable("fatal: bad ref".into())),
            repo(
                "me/docs",
                Refs::Unreadable("fatal: index file corrupt".into()),
            ),
        ]);
        assert_eq!(
            summarize(&conditions(&tree, Some("me/app: gh could not be run"))).as_deref(),
            Some("me/app: refs unreadable: fatal: bad ref (+2 more)"),
            "the count covers everything unsaid, gh included: a reader who is told about one \
             of three and nothing about the other two has been told the wrong thing"
        );
    }
}
