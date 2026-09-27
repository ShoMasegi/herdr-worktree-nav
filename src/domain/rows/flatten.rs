//! Flattening the tree into the rows the panes picker draws, in the shape herdr's own
//! session navigator uses: a row that did not match the filter itself is kept as context and
//! drawn dimmed rather than removed.
//!
//! Pure, so the shape of the list under every combination of folding, filtering, and hidden
//! ungrouped panes is covered by ordinary tests rather than by squinting at a terminal.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::domain::model::{CheckoutPath, PaneNode, RepoKey, Tree};
use crate::domain::rows::*;
use crate::port::AgentStatus;

/// Build the visible row list.
///
/// A match cascades downwards: matching a repository keeps its whole subtree, and matching a
/// worktree keeps the panes running on it. Those descendants keep their own `matched` flag
/// so they can be drawn as context. A pane that matches on its own pulls its headers along,
/// so a result is never shown without the context that explains where it is.
pub fn flatten(tree: &Tree, options: &ViewOptions) -> Vec<Row> {
    let query = options.query.trim();
    let pattern = (!query.is_empty())
        .then(|| Pattern::parse(query, CaseMatching::Smart, Normalization::Smart));
    let filtering = options.filtering();

    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    // `None` means "did not match"; with no query everything matches with score 0, which
    // leaves the tree in its natural order.
    let mut score = |haystack: &str| match &pattern {
        None => Some(0),
        Some(pattern) => pattern.score(Utf32Str::new(haystack, &mut buf), &mut matcher),
    };
    let state_ok = |status: AgentStatus| match options.state_filter {
        None => true,
        Some(filter) => filter.matches(status),
    };

    // Built per group first so that, while filtering, groups and worktrees can be ordered by
    // how well they matched. Fuzzy matching is permissive enough that an unrelated
    // repository often matches weakly, and it must not sit above the real answer.
    let mut groups: Vec<(u32, Vec<Row>)> = Vec::new();

    for (repo_index, repo) in tree.repos.iter().enumerate() {
        let panes: Vec<&PaneNode> = repo
            .worktrees
            .iter()
            .flat_map(|worktree| worktree.panes.iter())
            .collect();
        let repo_status = aggregate(panes.iter().map(|pane| pane.agent_status));
        let repo_score = score(&repo.display_name);
        let repo_matches = repo_score.is_some() && state_ok(repo_status);
        // A matching repository carries its subtree along, but only for a text query. A
        // state filter is a question about individual agents: a repository holding one
        // blocked agent must not present all its idle ones as blocked too.
        let cascade = repo_matches && options.state_filter.is_none();

        let mut subtrees: Vec<(u32, Vec<Row>)> = Vec::new();
        {
            for (worktree_index, worktree) in repo.worktrees.iter().enumerate() {
                if options.hide_worktrees_without_panes
                    && options.sweep.is_none()
                    && worktree.panes.is_empty()
                {
                    continue;
                }
                let worktree_haystack = format!("{} {}", repo.display_name, worktree.label());
                let worktree_status =
                    aggregate(worktree.panes.iter().map(|pane| pane.agent_status));
                let own_score = score(&worktree_haystack);
                let worktree_matches =
                    cascade || (own_score.is_some() && state_ok(worktree_status));
                let mut best = own_score.unwrap_or(0).max(repo_score.unwrap_or(0));

                let mut pane_rows = Vec::new();
                for (pane_index, pane) in worktree.panes.iter().enumerate() {
                    let haystack = format!(
                        "{} {}",
                        pane.display_name.as_deref().unwrap_or_default(),
                        pane.pane_id
                    );
                    let pane_score = score(&haystack);
                    let pane_matches = pane_score.is_some() && state_ok(pane.agent_status);
                    // A state filter narrows hard: an agent that is not in that state is
                    // not context, it is noise. A text query keeps the subtree of a match.
                    let keep = if options.state_filter.is_some() {
                        pane_matches
                    } else {
                        worktree_matches || pane_matches
                    };
                    if !keep {
                        continue;
                    }
                    best = best.max(pane_score.unwrap_or(0));
                    pane_rows.push(pane_row(
                        RowRef::Pane(repo_index, worktree_index, pane_index),
                        2,
                        pane,
                        pane_matches,
                    ));
                }

                if !worktree_matches && pane_rows.is_empty() {
                    continue;
                }
                let path = CheckoutPath::of(worktree);
                let mut subtree = vec![Row {
                    reference: RowRef::Worktree(repo_index, worktree_index),
                    depth: 1,
                    name: Some(worktree.label().to_string()),
                    panes: 0,
                    // The raw path. Writing it the way the user's shell would is
                    // `ui::words::abbreviate`'s, because `~` is a way of showing a path
                    // rather than a fact about one.
                    meta: worktree.checkout_path.clone(),
                    status: worktree_status,
                    is_idle: worktree.panes.is_empty(),
                    is_removing: options.removing.contains(&path),
                    working_tree: options.working_trees.get(&path).copied(),
                    track: worktree.track,
                    is_current: false,
                    matched: worktree_matches,
                    sweep: options
                        .sweep
                        .as_ref()
                        .and_then(|marks| marks.get(&(RepoKey::of(repo), path)).cloned()),
                }];
                subtree.append(&mut pane_rows);
                subtrees.push((best, subtree));
            }
        }

        if options.hide_worktrees_without_panes && options.sweep.is_none() && panes.is_empty() {
            continue;
        }
        if filtering && !repo_matches && subtrees.is_empty() {
            continue;
        }
        if filtering {
            // Stable, so equally-scoring worktrees keep primary-first order.
            subtrees.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        let best = subtrees
            .iter()
            .map(|(score, _)| *score)
            .max()
            .unwrap_or(0)
            .max(repo_score.unwrap_or(0));

        let mut group = vec![Row {
            reference: RowRef::Repo(repo_index),
            depth: 0,
            name: Some(repo.display_name.clone()),
            panes: panes.len(),
            // The main checkout listed directly below carries the same path.
            meta: String::new(),
            status: repo_status,
            is_idle: false,
            is_removing: false,
            working_tree: None,
            track: None,
            is_current: panes.iter().any(|pane| pane.focused),
            matched: repo_matches,
            sweep: None,
        }];
        for (_, mut subtree) in subtrees {
            group.append(&mut subtree);
        }
        groups.push((best, group));
    }

    if filtering {
        groups.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    }
    let mut rows: Vec<Row> = groups.into_iter().flat_map(|(_, group)| group).collect();

    // Panes in no repository are always listed. They are still panes, and a picker that
    // hides some of them makes you wonder which.
    {
        let mut panes = Vec::new();
        for (index, pane) in tree.ungrouped.iter().enumerate() {
            let haystack = format!(
                "{} {}",
                pane.display_name.as_deref().unwrap_or_default(),
                pane.pane_id
            );
            let matched = score(&haystack).is_some() && state_ok(pane.agent_status);
            if !matched {
                continue;
            }
            panes.push(pane_row(RowRef::Ungrouped(index), 1, pane, true));
        }
        if !panes.is_empty() {
            rows.push(Row {
                reference: RowRef::UngroupedRepo,
                depth: 0,
                // The group of panes no repository holds. It has no name of its own —
                // what it is called is `ui::words::row_label`'s to say.
                name: None,
                panes: tree.ungrouped.len(),
                meta: String::new(),
                status: aggregate(tree.ungrouped.iter().map(|pane| pane.agent_status)),
                is_idle: false,
                is_removing: false,
                working_tree: None,
                track: None,
                is_current: tree.ungrouped.iter().any(|pane| pane.focused),
                matched: true,
                sweep: None,
            });
            rows.append(&mut panes);
        }
    }

    rows
}

fn pane_row(reference: RowRef, depth: u8, pane: &PaneNode, matched: bool) -> Row {
    Row {
        reference,
        depth,
        name: pane.display_name.clone(),
        panes: 0,
        meta: pane.pane_id.clone(),
        status: pane.agent_status,
        is_idle: false,
        is_removing: false,
        working_tree: None,
        track: None,
        is_current: pane.focused,
        matched,
        sweep: None,
    }
}

/// The state a parent row shows: the most urgent one anything under it is in.
fn aggregate(statuses: impl Iterator<Item = AgentStatus>) -> AgentStatus {
    let mut best = AgentStatus::Unknown;
    for status in statuses {
        if urgency(status) > urgency(best) {
            best = status;
        }
    }
    best
}

fn urgency(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Working => 3,
        AgentStatus::Done => 2,
        AgentStatus::Idle => 1,
        AgentStatus::Unknown => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{Refs, RepoNode, WorktreeNode};
    use crate::domain::rows::fixtures::*;
    use crate::domain::sweep::Mark;
    use std::collections::BTreeMap;

    #[test]
    fn lays_the_tree_out_as_repo_worktree_pane_with_a_count_on_the_group() {
        assert_eq!(
            names(&flatten(&tree(), &ViewOptions::default())),
            [
                "me/app",
                "  main",
                "    claude",
                "    -",
                "  feat/login",
                "    codex",
                "  fix/crash",
                "me/site",
                "  develop",
                "    claude",
                "-",
                "  -",
            ]
        );
    }

    #[test]
    fn the_meta_column_says_where_the_thing_is() {
        let rows = flatten(&tree(), &ViewOptions::default());
        assert_eq!(find(&rows, "main").meta, "/wt/main");
        assert_eq!(find(&rows, "feat/login").meta, "/wt/feat-login");
        assert_eq!(find(&rows, "claude").meta, "w1:p1");
        assert_eq!(
            rows.iter()
                .find(|row| row.reference == RowRef::Pane(0, 0, 1))
                .expect("the pane herdr tracks no agent in")
                .meta,
            "w1:p2",
            "a pane with no name is still addressed by its id"
        );
    }

    #[test]
    fn a_repository_leaves_its_path_to_the_checkout_below_it() {
        // The main checkout sits directly under it with the same path.
        let rows = flatten(&tree(), &ViewOptions::default());
        assert_eq!(find(&rows, "me/app").meta, "");
    }

    #[test]
    fn a_checkout_with_nothing_running_is_flagged_beside_its_name() {
        // Its meta column is taken by the path, so the note goes next to the label.
        let rows = flatten(&tree(), &ViewOptions::default());
        assert!(find(&rows, "fix/crash").is_idle);
        assert_eq!(find(&rows, "fix/crash").meta, "/wt/fix-crash");
        assert!(!find(&rows, "main").is_idle);
    }

    #[test]
    fn worktrees_without_panes_can_be_hidden() {
        let rows = flatten(
            &tree(),
            &ViewOptions {
                hide_worktrees_without_panes: true,
                ..Default::default()
            },
        );
        assert!(!names(&rows).contains(&"  fix/crash".to_string()));
        assert!(names(&rows).contains(&"    codex".to_string()));
    }

    #[test]
    fn a_repository_with_no_panes_disappears_with_its_worktrees() {
        let mut tree = tree();
        tree.repos[1].worktrees[0].panes.clear();
        let rows = flatten(
            &tree,
            &ViewOptions {
                hide_worktrees_without_panes: true,
                ..Default::default()
            },
        );
        assert!(!names(&rows).contains(&"me/site".to_string()));
    }

    #[test]
    fn a_sweep_shows_worktrees_without_panes_even_when_the_ordinary_view_hides_them() {
        let rows = flatten(
            &tree(),
            &ViewOptions {
                hide_worktrees_without_panes: true,
                sweep: Some(BTreeMap::new()),
                ..Default::default()
            },
        );
        assert!(names(&rows).contains(&"  fix/crash".to_string()));
    }

    #[test]
    fn a_checkout_being_removed_says_that_instead_of_saying_it_is_empty() {
        // The removal runs in a process of its own, so the row has to say what is happening
        // to it for as long as the picker is up.
        let options = ViewOptions {
            removing: vec![CheckoutPath::for_test("/wt/fix-crash")],
            ..Default::default()
        };
        let rows = flatten(&tree(), &options);
        assert!(find(&rows, "fix/crash").is_removing);
        assert!(!find(&rows, "feat/login").is_removing);
    }

    #[test]
    fn a_parent_shows_the_most_urgent_state_underneath_it() {
        let rows = flatten(&tree(), &ViewOptions::default());
        // main holds a working agent and a plain shell; working wins over nothing.
        assert_eq!(find(&rows, "main").status, AgentStatus::Working);
        // The repository also holds an idle agent, but working is more urgent.
        assert_eq!(find(&rows, "me/app").status, AgentStatus::Working);
        assert_eq!(find(&rows, "me/site").status, AgentStatus::Blocked);
        assert_eq!(find(&rows, "fix/crash").status, AgentStatus::Unknown);
    }

    #[test]
    fn the_focused_pane_and_the_repository_holding_it_are_marked_as_current() {
        let mut tree = tree();
        tree.repos[1].worktrees[0].panes[0].focused = true;
        let rows = flatten(&tree, &ViewOptions::default());
        assert!(find(&rows, "me/site").is_current);
        assert!(
            rows.iter()
                .find(|r| r.reference == RowRef::Pane(1, 0, 0))
                .unwrap()
                .is_current
        );
        assert!(!find(&rows, "me/app").is_current);
        // The worktree level is never marked, matching herdr's tabs.
        assert!(!find(&rows, "develop").is_current);
    }

    #[test]
    fn a_match_keeps_the_whole_subtree_and_marks_the_rest_as_context() {
        let options = ViewOptions {
            query: "login".into(),
            ..Default::default()
        };
        let rows = flatten(&tree(), &options);
        assert_eq!(names(&rows), ["me/app", "  feat/login", "    codex"]);
        assert!(find(&rows, "feat/login").matched);
        // Kept so the result has context, but it is not itself a result.
        assert!(!find(&rows, "me/app").matched);
        // Carried in by its worktree rather than by matching, so it is context too.
        assert!(!find(&rows, "codex").matched);
    }

    #[test]
    fn everything_is_a_match_when_nothing_is_being_filtered() {
        let rows = flatten(&tree(), &ViewOptions::default());
        assert!(rows.iter().all(|row| row.matched));
    }

    #[test]
    fn a_state_filter_narrows_hard_instead_of_keeping_context_panes() {
        let options = ViewOptions {
            state_filter: Some(StateFilter::Blocked),
            ..Default::default()
        };
        let rows = flatten(&tree(), &options);
        assert_eq!(names(&rows), ["me/site", "  develop", "    claude"]);
    }

    #[test]
    fn a_state_filter_does_not_cascade_a_repositorys_match_onto_its_quiet_worktrees() {
        // me/app aggregates to "working" because of main, but feat/login is idle and
        // fix/crash holds nothing. Only the branch that is actually working may survive.
        let options = ViewOptions {
            state_filter: Some(StateFilter::Working),
            ..Default::default()
        };
        assert_eq!(
            names(&flatten(&tree(), &options)),
            ["me/app", "  main", "    claude"]
        );
    }

    #[test]
    fn a_state_filter_that_matches_nothing_leaves_an_empty_list() {
        let options = ViewOptions {
            state_filter: Some(StateFilter::Done),
            ..Default::default()
        };
        assert!(flatten(&tree(), &options).is_empty());
    }

    #[test]
    fn panes_outside_a_repository_form_their_own_group() {
        let rows = flatten(&tree(), &ViewOptions::default());
        let group = rows
            .iter()
            .find(|row| row.reference == RowRef::UngroupedRepo)
            .expect("the group of panes no repository holds");
        assert!(
            group.reference.is_group(),
            "it heads a section and takes a blank line above it, like a repository"
        );
        assert_eq!(
            group.panes, 1,
            "it counts what it holds, which is what names it"
        );
        assert_eq!(
            rows.last().unwrap().name,
            None,
            "herdr tracks no agent in it, so it has no name of its own"
        );
    }

    #[test]
    fn puts_the_best_match_first_because_fuzzy_matching_is_permissive() {
        // "harken" is a subsequence of plenty of unrelated text, so an exact-ish match has
        // to outrank the incidental ones. herdr's navigator keeps session order instead,
        // but its rows are ordered by something the user already knows; ours are not.
        let tree = Tree {
            repos: vec![
                RepoNode {
                    repo_key: "/src/hbr/.git".into(),
                    repo_root: "/src/lin".into(),
                    display_name: "me/harbour-backend".into(),
                    refs: Refs::Read,
                    worktrees: vec![worktree("feat/hbr-51-grant-table", vec![])],
                },
                RepoNode {
                    repo_key: "/src/harken/.git".into(),
                    repo_root: "/src/harken".into(),
                    display_name: "me/harken".into(),
                    refs: Refs::Read,
                    worktrees: vec![worktree("main", vec![])],
                },
            ],
            ungrouped: vec![],
            ..Default::default()
        };
        let options = ViewOptions {
            query: "harken".into(),
            ..Default::default()
        };
        assert_eq!(
            flatten(&tree, &options)[0].name.as_deref(),
            Some("me/harken")
        );
    }

    #[test]
    fn a_mark_is_looked_up_by_the_rows_own_repository_and_not_by_its_path_alone() {
        // Two repositories list one path — `me/site` once had a worktree where `me/app` has
        // a live checkout — and each row shows its own repository's answer.
        use crate::domain::sweep::{Half, Reason};
        let mut tree = tree();
        tree.repos[1].worktrees.push(WorktreeNode {
            branch: Some("chore/deps".into()),
            ..worktree("fix/crash", vec![])
        });
        let at = |repo: usize| {
            (
                RepoKey::of(&tree.repos[repo]),
                CheckoutPath::for_test("/wt/fix-crash"),
            )
        };
        let options = ViewOptions {
            sweep: Some(BTreeMap::from([
                (at(0), Mark::Unjudged(Half::Refs)),
                (at(1), Mark::Going(Reason::Gone)),
            ])),
            ..Default::default()
        };
        let rows = flatten(&tree, &options);
        assert_eq!(
            find(&rows, "fix/crash").sweep,
            Some(Mark::Unjudged(Half::Refs))
        );
        assert_eq!(
            find(&rows, "chore/deps").sweep,
            Some(Mark::Going(Reason::Gone))
        );
    }
}
