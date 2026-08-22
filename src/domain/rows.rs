//! Flattening the tree into the list of rows the panes view draws, and filtering it.
//!
//! Pure, so the shape of the list under every combination of collapsing, filtering, and
//! hidden ungrouped panes is covered by ordinary tests rather than by squinting at a
//! terminal.

use std::collections::HashSet;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::domain::model::Tree;
use crate::port::AgentStatus;

/// Which node of the tree a row stands for. Indices point into [`Tree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRef {
    Repo(usize),
    Worktree(usize, usize),
    Pane(usize, usize, usize),
    /// The "not in any repository" separator.
    UngroupedHeader,
    Ungrouped(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub reference: RowRef,
    /// Nesting level, in tree steps rather than columns.
    pub indent: u16,
    /// The name the row is identified by.
    pub primary: String,
    /// Dimmed detail: a checkout path, or a pane id.
    pub secondary: String,
    pub status: Option<AgentStatus>,
    /// Whether the cursor may land here. Only the ungrouped separator may not.
    pub selectable: bool,
}

/// Shown when a pane has no agent and no terminal title, which is what a plain shell looks
/// like in a herdr snapshot.
const UNNAMED_PANE: &str = "shell";

#[derive(Debug, Clone, Default)]
pub struct ViewOptions {
    /// Show panes that are not inside any git work tree.
    pub show_ungrouped: bool,
    /// `repo_key`s the user has folded away.
    pub collapsed: HashSet<String>,
    pub query: String,
}

/// Build the visible row list.
///
/// A match cascades downwards: matching a repository shows everything in it, and matching a
/// worktree shows the panes running on it. A pane that matches on its own pulls its
/// worktree and repository headers along so it is never shown without context.
pub fn flatten(tree: &Tree, options: &ViewOptions) -> Vec<Row> {
    let query = options.query.trim();
    let pattern = (!query.is_empty())
        .then(|| Pattern::parse(query, CaseMatching::Smart, Normalization::Smart));
    let filtering = pattern.is_some();

    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    // `None` means "did not match"; with no query everything matches with score 0, which
    // leaves the tree in its natural order.
    let mut score = |haystack: &str| match &pattern {
        None => Some(0),
        Some(pattern) => pattern.score(Utf32Str::new(haystack, &mut buf), &mut matcher),
    };

    // Built per repository first so that, while filtering, repositories and worktrees can
    // be ordered by how well they matched. Fuzzy matching is permissive enough that an
    // unrelated repository often matches weakly, and it must not sit above the real answer.
    let mut groups: Vec<(u32, Vec<Row>)> = Vec::new();

    for (repo_index, repo) in tree.repos.iter().enumerate() {
        let repo_score = score(&repo.display_name);
        let repo_matches = repo_score.is_some();
        // Filtering overrides collapsing: a fold set earlier should not hide what the user
        // is searching for now.
        let collapsed = !filtering && options.collapsed.contains(&repo.repo_key);

        let mut subtrees: Vec<(u32, Vec<Row>)> = Vec::new();
        if !collapsed {
            for (worktree_index, worktree) in repo.worktrees.iter().enumerate() {
                let worktree_haystack = format!("{} {}", repo.display_name, worktree.label());
                let own_score = score(&worktree_haystack);
                let worktree_matches = repo_matches || own_score.is_some();
                let mut best = own_score.unwrap_or(0).max(repo_score.unwrap_or(0));

                let mut panes = Vec::new();
                for (pane_index, pane) in worktree.panes.iter().enumerate() {
                    let haystack = format!(
                        "{} {} {}",
                        worktree_haystack,
                        pane.display_name.as_deref().unwrap_or_default(),
                        pane.pane_id
                    );
                    let pane_score = score(&haystack);
                    if !worktree_matches && pane_score.is_none() {
                        continue;
                    }
                    best = best.max(pane_score.unwrap_or(0));
                    panes.push(Row {
                        reference: RowRef::Pane(repo_index, worktree_index, pane_index),
                        indent: 2,
                        primary: pane
                            .display_name
                            .clone()
                            .unwrap_or_else(|| UNNAMED_PANE.to_string()),
                        secondary: pane.pane_id.clone(),
                        status: Some(pane.agent_status),
                        selectable: true,
                    });
                }

                if !worktree_matches && panes.is_empty() {
                    continue;
                }
                let mut subtree = Vec::new();
                subtree.push(Row {
                    reference: RowRef::Worktree(repo_index, worktree_index),
                    indent: 1,
                    primary: worktree.label().to_string(),
                    secondary: worktree.checkout_path.clone(),
                    status: None,
                    selectable: true,
                });
                subtree.append(&mut panes);
                subtrees.push((best, subtree));
            }
        }

        if filtering && !repo_matches && subtrees.is_empty() {
            continue;
        }
        if filtering {
            // Stable, so equally-scoring worktrees keep primary-first order.
            subtrees.sort_by(|a, b| b.0.cmp(&a.0));
        }
        let best = subtrees
            .iter()
            .map(|(score, _)| *score)
            .max()
            .unwrap_or(0)
            .max(repo_score.unwrap_or(0));

        let mut group = vec![Row {
            reference: RowRef::Repo(repo_index),
            indent: 0,
            primary: repo.display_name.clone(),
            secondary: repo.repo_root.clone(),
            status: None,
            selectable: true,
        }];
        for (_, mut subtree) in subtrees {
            group.append(&mut subtree);
        }
        groups.push((best, group));
    }

    if filtering {
        groups.sort_by(|a, b| b.0.cmp(&a.0));
    }
    let mut rows: Vec<Row> = groups.into_iter().flat_map(|(_, group)| group).collect();

    if options.show_ungrouped {
        let mut panes = Vec::new();
        for (index, pane) in tree.ungrouped.iter().enumerate() {
            let haystack = format!(
                "{} {}",
                pane.display_name.as_deref().unwrap_or_default(),
                pane.pane_id
            );
            if score(&haystack).is_none() {
                continue;
            }
            panes.push(Row {
                reference: RowRef::Ungrouped(index),
                indent: 1,
                primary: pane
                    .display_name
                    .clone()
                    .unwrap_or_else(|| UNNAMED_PANE.to_string()),
                secondary: pane.pane_id.clone(),
                status: Some(pane.agent_status),
                selectable: true,
            });
        }
        if !panes.is_empty() {
            rows.push(Row {
                reference: RowRef::UngroupedHeader,
                indent: 0,
                primary: "not in any repository".to_string(),
                secondary: String::new(),
                status: None,
                selectable: false,
            });
            rows.append(&mut panes);
        }
    }

    rows
}

/// Index of the first selectable row at or after `from`, wrapping to the start.
pub fn next_selectable(rows: &[Row], from: usize) -> Option<usize> {
    let len = rows.len();
    (0..len).find_map(|offset| {
        let index = (from + offset) % len;
        rows[index].selectable.then_some(index)
    })
}

/// Index of the first selectable row at or before `from`, wrapping to the end.
pub fn previous_selectable(rows: &[Row], from: usize) -> Option<usize> {
    let len = rows.len();
    (0..len).find_map(|offset| {
        let index = (from + len - offset) % len;
        rows[index].selectable.then_some(index)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{PaneNode, RepoNode, WorktreeNode};

    fn pane(id: &str, name: Option<&str>) -> PaneNode {
        PaneNode {
            pane_id: id.to_string(),
            workspace_id: id.split(':').next().unwrap().to_string(),
            tab_id: format!("{}:t1", id.split(':').next().unwrap()),
            display_name: name.map(str::to_string),
            agent_status: AgentStatus::Idle,
            focused: false,
        }
    }

    fn worktree(branch: &str, panes: Vec<PaneNode>) -> WorktreeNode {
        WorktreeNode {
            branch: Some(branch.to_string()),
            checkout_path: format!("/wt/{}", branch.replace('/', "-")),
            is_primary: branch == "main",
            open_workspace_id: None,
            panes,
        }
    }

    /// Two repositories: `me/app` on main and feat/login, and `me/site` on main.
    fn tree() -> Tree {
        Tree {
            repos: vec![
                RepoNode {
                    repo_key: "/src/app/.git".into(),
                    repo_root: "/src/app".into(),
                    display_name: "me/app".into(),
                    worktrees: vec![
                        worktree("main", vec![pane("w1:p1", Some("claude"))]),
                        worktree("feat/login", vec![pane("w2:p1", Some("codex"))]),
                    ],
                },
                RepoNode {
                    repo_key: "/src/site/.git".into(),
                    repo_root: "/src/site".into(),
                    display_name: "me/site".into(),
                    worktrees: vec![worktree("main", vec![pane("w3:p1", None)])],
                },
            ],
            ungrouped: vec![pane("w9:p1", None)],
        }
    }

    fn labels(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}{}", "  ".repeat(r.indent as usize), r.primary))
            .collect()
    }

    #[test]
    fn lays_the_tree_out_as_repo_worktree_pane() {
        let rows = flatten(&tree(), &ViewOptions::default());
        assert_eq!(
            labels(&rows),
            [
                "me/app",
                "  main",
                "    claude",
                "  feat/login",
                "    codex",
                "me/site",
                "  main",
                "    shell",
            ]
        );
    }

    #[test]
    fn names_a_pane_with_no_agent_and_no_title_rather_than_leaving_it_blank() {
        let rows = flatten(&tree(), &ViewOptions::default());
        let unnamed = rows.iter().find(|r| r.secondary == "w3:p1").unwrap();
        assert_eq!(unnamed.primary, "shell");
    }

    #[test]
    fn hides_ungrouped_panes_unless_asked_for_them() {
        let hidden = flatten(&tree(), &ViewOptions::default());
        assert!(!labels(&hidden).iter().any(|l| l.contains("not in any")));

        let shown = flatten(
            &tree(),
            &ViewOptions {
                show_ungrouped: true,
                ..Default::default()
            },
        );
        assert_eq!(
            labels(&shown).last().unwrap().trim(),
            "shell",
            "the ungrouped pane should be the final row"
        );
        assert!(shown
            .iter()
            .any(|r| r.reference == RowRef::UngroupedHeader && !r.selectable));
    }

    #[test]
    fn collapsing_a_repo_hides_its_children_but_keeps_its_header() {
        let options = ViewOptions {
            collapsed: HashSet::from(["/src/app/.git".to_string()]),
            ..Default::default()
        };
        assert_eq!(
            labels(&flatten(&tree(), &options)),
            ["me/app", "me/site", "  main", "    shell"]
        );
    }

    #[test]
    fn matching_a_worktree_brings_the_panes_running_on_it_along() {
        let options = ViewOptions {
            query: "login".into(),
            ..Default::default()
        };
        assert_eq!(
            labels(&flatten(&tree(), &options)),
            ["me/app", "  feat/login", "    codex"]
        );
    }

    #[test]
    fn matching_a_pane_pulls_its_headers_along_so_it_is_never_shown_without_context() {
        let options = ViewOptions {
            query: "codex".into(),
            ..Default::default()
        };
        assert_eq!(
            labels(&flatten(&tree(), &options)),
            ["me/app", "  feat/login", "    codex"]
        );
    }

    #[test]
    fn matching_a_repo_shows_everything_inside_it() {
        let options = ViewOptions {
            query: "site".into(),
            ..Default::default()
        };
        assert_eq!(
            labels(&flatten(&tree(), &options)),
            ["me/site", "  main", "    shell"]
        );
    }

    #[test]
    fn filtering_overrides_a_fold_so_the_search_target_is_never_hidden() {
        let options = ViewOptions {
            collapsed: HashSet::from(["/src/app/.git".to_string()]),
            query: "codex".into(),
            ..Default::default()
        };
        assert_eq!(
            labels(&flatten(&tree(), &options)),
            ["me/app", "  feat/login", "    codex"]
        );
    }

    #[test]
    fn puts_the_best_match_first_because_fuzzy_matching_is_permissive() {
        // "harken" is a subsequence of plenty of unrelated text, so an exact-ish match has
        // to outrank the incidental ones rather than sitting below them in tree order.
        let tree = Tree {
            repos: vec![
                RepoNode {
                    repo_key: "/src/hbr/.git".into(),
                    repo_root: "/src/lin".into(),
                    display_name: "me/harbour-backend".into(),
                    worktrees: vec![worktree("feat/hbr-51-grant-table", vec![])],
                },
                RepoNode {
                    repo_key: "/src/harken/.git".into(),
                    repo_root: "/src/harken".into(),
                    display_name: "me/harken".into(),
                    worktrees: vec![worktree("main", vec![])],
                },
            ],
            ungrouped: vec![],
        };
        let options = ViewOptions {
            query: "harken".into(),
            ..Default::default()
        };
        let rows = flatten(&tree, &options);
        assert_eq!(rows[0].primary, "me/harken");
    }

    #[test]
    fn a_query_that_matches_nothing_produces_no_rows() {
        let options = ViewOptions {
            query: "zzzznope".into(),
            show_ungrouped: true,
            ..Default::default()
        };
        assert!(flatten(&tree(), &options).is_empty());
    }

    #[test]
    fn cursor_movement_skips_the_unselectable_separator_and_wraps() {
        let rows = flatten(
            &tree(),
            &ViewOptions {
                show_ungrouped: true,
                ..Default::default()
            },
        );
        let separator = rows
            .iter()
            .position(|r| r.reference == RowRef::UngroupedHeader)
            .unwrap();

        assert_eq!(next_selectable(&rows, separator), Some(separator + 1));
        assert_eq!(previous_selectable(&rows, separator), Some(separator - 1));
        // Past the end, wrap back to the first row.
        assert_eq!(next_selectable(&rows, rows.len() - 1), Some(rows.len() - 1));
        assert_eq!(previous_selectable(&rows, 0), Some(0));
    }

    #[test]
    fn cursor_movement_on_an_empty_list_has_no_answer_rather_than_panicking() {
        assert_eq!(next_selectable(&[], 0), None);
        assert_eq!(previous_selectable(&[], 0), None);
    }
}
