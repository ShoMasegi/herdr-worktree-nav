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

use std::collections::BTreeMap;

use crate::domain::model::{Refs, Tree};

/// One thing the picker has to say about the list it is showing.
///
/// A value rather than a sentence: which of these hold is derived every frame, and what
/// each one reads as on a prompt line that may have no room for it is
/// [`ui::words`](crate::ui::words)'s to decide. There is deliberately no severity here yet.
/// Every condition produced today is about one repository or about the reading as a whole,
/// so an ordering field would be a guess; the order is the order they are gathered in, and
/// [`conditions`] says what that order means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// The reading that built the list failed, so the rows may be behind. Carries the words
    /// of whatever refused, which the picker has no better account of.
    Stale(String),
    /// git could not be asked which repository these panes are in, so they draw as though
    /// they were in none. Counted by what git said rather than listed one per pane: a `git`
    /// that is not on the path fails for every pane in the session with one sentence, and
    /// what a reader needs is the words and how much of the session they cost.
    Unplaced { panes: usize, words: String },
    /// herdr would not list this repository's worktrees, so it has no rows at all. Carries
    /// herdr's own words and the name [`Unlisted::name`](crate::domain::model::Unlisted::name)
    /// makes out of the key, which is the only name this side has.
    Unlisted { repo: String, words: String },
    /// git would not read this repository's refs, so every track marker in it is missing.
    RefsUnreadable { repo: String, words: String },
    /// What could not be asked of `gh` in this sweep, already counted by
    /// [`app::settled`](crate::app::settled) because which repositories were asked is not
    /// something the tree knows.
    SweepTrouble(String),
}

/// Everything that is wrong right now, worst first.
///
/// After a list that may be behind: panes that could not be placed at all, then a repository
/// with no rows at all, then a repository whose rows are missing their markers, then what
/// `gh` said. Each is a larger question than the one after it — a reader whose whole session
/// is ungrouped is not helped by being told about one repository's refs, and one who cannot
/// see a repository at all is not helped by being told about another one's markers. The
/// markers are about every row of the repository and are true sweep or no sweep; `gh` is
/// asked only during a sweep, and only about the half git could not decide.
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
    // Counted by what git said rather than listed one per pane: a `git` that is not on the
    // path fails for every pane git is asked about, and the same sentence once per pane is
    // not more information than the sentence and a number. Most panes first, since the
    // prompt line holds one of these and the one that cost the most of the session is the
    // one worth its room; words in order among equals, so two readings of one session say
    // the same thing.
    let mut by_words: BTreeMap<&str, usize> = BTreeMap::new();
    for words in tree.trouble.unplaced.values() {
        *by_words.entry(words.as_str()).or_default() += 1;
    }
    let mut by_cost: Vec<(&str, usize)> = by_words.into_iter().collect();
    by_cost.sort_by(|(_, a), (_, b)| b.cmp(a));
    conditions.extend(
        by_cost
            .into_iter()
            .map(|(words, panes)| Condition::Unplaced {
                panes,
                words: words.to_string(),
            }),
    );
    conditions.extend(
        tree.trouble
            .unlisted
            .iter()
            .map(|repo| Condition::Unlisted {
                repo: repo.name().to_string(),
                words: repo.words.clone(),
            }),
    );
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
    use crate::domain::model::{RepoNode, Tree, Unlisted};

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
    fn panes_git_would_not_answer_about_are_counted_by_what_it_said() {
        // A `git` that is not on the path fails for every pane at once, and one condition
        // is the whole of it. What a reader needs is the words and how much of the session
        // they cost, not the same line once per pane.
        let mut tree = tree(vec![repo("me/app", Refs::Read)]);
        assert_eq!(conditions(&tree, None, None), Vec::new());

        let refused = "git could not be run: no such file or directory (`git rev-parse`)";
        for pane in ["w1:p1", "w2:p1"] {
            tree.trouble
                .unplaced
                .insert(pane.to_string(), refused.to_string());
        }
        assert_eq!(
            conditions(&tree, None, None),
            vec![Condition::Unplaced {
                panes: 2,
                words: refused.into(),
            }]
        );

        // A pane that failed for another reason is its own condition: two shapes of
        // failure are two things to fix. The one that cost more of the session comes first,
        // whatever its words sort as — `fatal` is ahead of `git` in the alphabet, and a
        // missing git behind one odd pane is the sentence a reader most needs to see.
        let dubious = "fatal: detected dubious ownership (`git rev-parse`)";
        let warned = "warning: unable to access '/etc/gitconfig' (`git rev-parse`)";
        tree.trouble
            .unplaced
            .insert("w3:p1".to_string(), dubious.to_string());
        tree.trouble
            .unplaced
            .insert("w4:p1".to_string(), warned.to_string());
        assert_eq!(
            conditions(&tree, None, None),
            vec![
                Condition::Unplaced {
                    panes: 2,
                    words: refused.into(),
                },
                // Two that cost the same are in the order of their words, so the line does
                // not change between two readings of one session.
                Condition::Unplaced {
                    panes: 1,
                    words: dubious.into(),
                },
                Condition::Unplaced {
                    panes: 1,
                    words: warned.into(),
                },
            ]
        );
    }

    #[test]
    fn a_session_that_could_not_be_grouped_is_gathered_ahead_of_everything_else() {
        // Each of these is a larger question than the one after it. A reader whose whole
        // session is ungrouped is not helped by being told about one repository's refs.
        let mut tree = tree(vec![repo(
            "me/app",
            Refs::Unreadable("fatal: index file corrupt".into()),
        )]);
        tree.trouble.unlisted.push(Unlisted {
            repo_key: "/src/old/.git".into(),
            words: "herdr rejected worktree.list: internal error".into(),
            panes: Default::default(),
        });
        tree.trouble.unplaced.insert(
            "w1:p1".to_string(),
            "git could not be run: no such file or directory (`git rev-parse`)".into(),
        );
        assert_eq!(
            conditions(&tree, None, None)[0],
            Condition::Unplaced {
                panes: 1,
                words: "git could not be run: no such file or directory (`git rev-parse`)".into(),
            }
        );
    }

    #[test]
    fn a_repository_herdr_would_not_list_is_named_with_herdrs_words() {
        // It has no rows to say it with — that is what "not listed" means — so a condition
        // is the only place it can be said at all.
        let mut tree = tree(vec![repo("me/app", Refs::Read)]);
        tree.trouble.unlisted.push(Unlisted {
            repo_key: "/src/old/.git".into(),
            words: "herdr rejected worktree.list: internal error".into(),
            panes: Default::default(),
        });
        assert_eq!(
            conditions(&tree, None, None),
            vec![Condition::Unlisted {
                repo: "old".into(),
                words: "herdr rejected worktree.list: internal error".into(),
            }]
        );
    }

    #[test]
    fn a_repository_that_is_not_there_is_gathered_ahead_of_one_whose_refs_were_not_read() {
        // "Which rows exist" before "what the rows say": a reader who cannot see a
        // repository at all is not helped by being told about another one's markers.
        let mut tree = tree(vec![repo(
            "me/app",
            Refs::Unreadable("fatal: index file corrupt".into()),
        )]);
        tree.trouble.unlisted.push(Unlisted {
            repo_key: "/src/old/.git".into(),
            words: "herdr rejected worktree.list: internal error".into(),
            panes: Default::default(),
        });
        assert_eq!(
            conditions(&tree, None, None),
            vec![
                Condition::Unlisted {
                    repo: "old".into(),
                    words: "herdr rejected worktree.list: internal error".into(),
                },
                Condition::RefsUnreadable {
                    repo: "me/app".into(),
                    words: "fatal: index file corrupt".into(),
                },
            ],
            "neither hides the other, and the larger question is first"
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
