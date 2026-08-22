//! Flattening the tree into the rows the panes picker draws, in the shape herdr's own
//! session navigator uses.
//!
//! A row carries what the navigator's rows carry: a depth for the tree glyphs, a label, a
//! right-hand `meta` column, an aggregate status, whether it is the row the session is
//! currently on, and whether it matched the active filter — a row that did not match itself
//! is kept as context and drawn dimmed rather than removed.
//!
//! Pure, so the shape of the list under every combination of folding, filtering, and hidden
//! ungrouped panes is covered by ordinary tests rather than by squinting at a terminal.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::domain::model::{PaneNode, Tree};
use crate::port::AgentStatus;

/// Which node of the tree a row stands for. Indices point into [`Tree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRef {
    Repo(usize),
    Worktree(usize, usize),
    Pane(usize, usize, usize),
    /// The group holding panes that are not inside any git work tree.
    UngroupedRepo,
    Ungrouped(usize),
}

impl RowRef {
    /// Whether this row heads a group — the navigator's "workspace" role, which is what
    /// gets the blank line above it.
    pub fn is_group(self) -> bool {
        matches!(self, RowRef::Repo(_) | RowRef::UngroupedRepo)
    }

    /// The navigator's "tab" role: a middle level that is neither a group nor a leaf.
    pub fn is_worktree(self) -> bool {
        matches!(self, RowRef::Worktree(..))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub reference: RowRef,
    /// Tree depth: 0 for a group, 1 for a worktree, 2 for a pane under one.
    pub depth: u8,
    pub label: String,
    /// The right-hand column: where the thing is. A checkout path for a worktree, a pane id
    /// for a pane, and for a repository its root — but only while it is folded, since the
    /// main checkout directly beneath otherwise repeats it.
    pub meta: String,
    pub status: AgentStatus,
    /// A worktree with no pane in it. Called out beside the label, since its meta column is
    /// taken by the checkout path.
    pub is_idle: bool,
    /// The row the session is currently on, marked with a caret in the gutter.
    pub is_current: bool,
    /// Whether this row matched the active filter, as opposed to being kept as ancestor
    /// context or as part of a matching group's subtree. Always true with no filter.
    pub matched: bool,
}

impl Row {
    /// Whether the cursor stops here.
    ///
    /// Only rows that stand for somewhere to go: a pane, and a checkout with nothing
    /// running in it. A repository is a heading, and a checkout that already has panes is
    /// answered by the panes listed directly under it — stopping on either would only make
    /// the arrow keys longer to press.
    pub fn is_selectable(&self) -> bool {
        match self.reference {
            RowRef::Pane(..) | RowRef::Ungrouped(_) => true,
            RowRef::Worktree(..) => self.is_idle,
            RowRef::Repo(_) | RowRef::UngroupedRepo => false,
        }
    }
}

/// One rendered line. Blank lines separate groups and cannot be selected — the same shape
/// herdr's navigator uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayLine {
    Spacer,
    Row(usize),
}

/// Interleave a blank line before every group but the first.
pub fn display_lines(rows: &[Row]) -> Vec<DisplayLine> {
    let mut lines = Vec::with_capacity(rows.len() * 2);
    for (index, row) in rows.iter().enumerate() {
        if row.reference.is_group() && !lines.is_empty() {
            lines.push(DisplayLine::Spacer);
        }
        lines.push(DisplayLine::Row(index));
    }
    lines
}

/// Narrow the list to one agent state, the way the navigator's `b`/`w`/`i`/`d` keys do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    Blocked,
    Working,
    Idle,
    Done,
}

impl StateFilter {
    pub fn label(self) -> &'static str {
        match self {
            StateFilter::Blocked => "blocked",
            StateFilter::Working => "working",
            StateFilter::Idle => "idle",
            StateFilter::Done => "done",
        }
    }

    pub fn status(self) -> AgentStatus {
        match self {
            StateFilter::Blocked => AgentStatus::Blocked,
            StateFilter::Working => AgentStatus::Working,
            StateFilter::Idle => AgentStatus::Idle,
            StateFilter::Done => AgentStatus::Done,
        }
    }

    fn matches(self, status: AgentStatus) -> bool {
        self.status() == status
    }
}

/// Shown when a pane has no agent and no terminal title, which is what a plain shell looks
/// like in a herdr snapshot.
const UNNAMED_PANE: &str = "shell";

#[derive(Debug, Clone, Default)]
pub struct ViewOptions {
    /// Show panes that are not inside any git work tree.
    pub show_ungrouped: bool,
    pub query: String,
    pub state_filter: Option<StateFilter>,
    /// The user's home directory, so paths can be shown as `~/...`. Passed in rather than
    /// read, because this module does not touch the environment.
    pub home: Option<String>,
}

impl ViewOptions {
    fn filtering(&self) -> bool {
        !self.query.trim().is_empty() || self.state_filter.is_some()
    }
}

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
                let mut subtree = vec![Row {
                    reference: RowRef::Worktree(repo_index, worktree_index),
                    depth: 1,
                    label: worktree.label().to_string(),
                    meta: abbreviate(&worktree.checkout_path, options.home.as_deref()),
                    status: worktree_status,
                    is_idle: worktree.panes.is_empty(),
                    is_current: false,
                    matched: worktree_matches,
                }];
                subtree.append(&mut pane_rows);
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
            depth: 0,
            label: format!("{} ({})", repo.display_name, panes.len()),
            // The main checkout listed directly below carries the same path.
            meta: String::new(),
            status: repo_status,
            is_idle: false,
            is_current: panes.iter().any(|pane| pane.focused),
            matched: repo_matches,
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
                label: format!("not in any repository ({})", tree.ungrouped.len()),
                meta: String::new(),
                status: aggregate(tree.ungrouped.iter().map(|pane| pane.agent_status)),
                is_idle: false,
                is_current: tree.ungrouped.iter().any(|pane| pane.focused),
                matched: true,
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
        label: pane
            .display_name
            .clone()
            .unwrap_or_else(|| UNNAMED_PANE.to_string()),
        meta: pane.pane_id.clone(),
        status: pane.agent_status,
        is_idle: false,
        is_current: pane.focused,
        matched,
    }
}

/// Show a path under the user's home as `~/...`. Popups are narrower than a full pane, and
/// sixteen characters of `/Users/someone` are the least useful part of a checkout path.
pub(crate) fn abbreviate(path: &str, home: Option<&str>) -> String {
    let Some(home) = home.filter(|home| !home.is_empty()) else {
        return path.to_string();
    };
    let home = home.trim_end_matches('/');
    match path.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
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

/// herdr's wording for an agent state. `None` when there is no agent to describe.
pub fn status_label(status: AgentStatus) -> Option<&'static str> {
    match status {
        AgentStatus::Blocked => Some("blocked"),
        AgentStatus::Working => Some("working"),
        AgentStatus::Done => Some("done"),
        AgentStatus::Idle => Some("idle"),
        AgentStatus::Unknown => None,
    }
}

/// Index of the first line the cursor may sit on, at or after `from`, wrapping to the
/// start. `None` when nothing in the list is selectable at all.
pub fn next_row(rows: &[Row], lines: &[DisplayLine], from: usize) -> Option<usize> {
    let len = lines.len();
    (0..len).find_map(|offset| {
        let index = (from + offset) % len;
        selectable(rows, lines, index).then_some(index)
    })
}

/// Index of the first line the cursor may sit on, at or before `from`, wrapping to the end.
pub fn previous_row(rows: &[Row], lines: &[DisplayLine], from: usize) -> Option<usize> {
    let len = lines.len();
    (0..len).find_map(|offset| {
        let index = (from + len - offset) % len;
        selectable(rows, lines, index).then_some(index)
    })
}

fn selectable(rows: &[Row], lines: &[DisplayLine], index: usize) -> bool {
    match lines.get(index) {
        Some(DisplayLine::Row(row)) => rows[*row].is_selectable(),
        // A blank line separating two groups, or nothing there at all.
        Some(DisplayLine::Spacer) | None => false,
    }
}

/// The breadcrumb shown under the list for the row the cursor is on.
///
/// This is where the checkout path lives. The navigator keeps its rows to a label and one
/// meta column and puts the fuller context here, so the list stays scannable and nothing is
/// actually lost.
pub fn detail(tree: &Tree, reference: RowRef) -> String {
    let parts: Vec<String> = match reference {
        RowRef::Repo(repo_index) => {
            let Some(repo) = tree.repos.get(repo_index) else {
                return String::new();
            };
            let panes: usize = repo.worktrees.iter().map(|w| w.panes.len()).sum();
            let worktrees = repo.worktrees.len();
            vec![
                repo.display_name.clone(),
                format!("{worktrees} {}", plural(worktrees, "worktree")),
                format!("{panes} {}", plural(panes, "pane")),
                repo.repo_root.clone(),
            ]
        }
        RowRef::Worktree(repo_index, worktree_index) => {
            let Some(repo) = tree.repos.get(repo_index) else {
                return String::new();
            };
            let Some(worktree) = repo.worktrees.get(worktree_index) else {
                return String::new();
            };
            let mut parts = vec![repo.display_name.clone(), worktree.label().to_string()];
            if worktree.is_primary {
                parts.push("main checkout".to_string());
            }
            parts.push(worktree.checkout_path.clone());
            parts
        }
        RowRef::Pane(repo_index, worktree_index, pane_index) => {
            let Some(repo) = tree.repos.get(repo_index) else {
                return String::new();
            };
            let Some(worktree) = repo.worktrees.get(worktree_index) else {
                return String::new();
            };
            let Some(pane) = worktree.panes.get(pane_index) else {
                return String::new();
            };
            let mut parts = vec![
                repo.display_name.clone(),
                worktree.label().to_string(),
                pane.pane_id.clone(),
            ];
            if let Some(label) = status_label(pane.agent_status) {
                parts.push(label.to_string());
            }
            parts.push(worktree.checkout_path.clone());
            parts
        }
        RowRef::UngroupedRepo => vec![
            "not inside any git work tree".to_string(),
            format!(
                "{} {}",
                tree.ungrouped.len(),
                plural(tree.ungrouped.len(), "pane")
            ),
        ],
        RowRef::Ungrouped(index) => {
            let Some(pane) = tree.ungrouped.get(index) else {
                return String::new();
            };
            let mut parts = vec![
                pane.display_name
                    .clone()
                    .unwrap_or_else(|| UNNAMED_PANE.to_string()),
                pane.pane_id.clone(),
            ];
            if let Some(label) = status_label(pane.agent_status) {
                parts.push(label.to_string());
            }
            parts
        }
    };
    parts.join(" \u{b7} ")
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{RepoNode, WorktreeNode};

    fn pane(id: &str, name: Option<&str>, status: AgentStatus) -> PaneNode {
        let workspace = id.split(':').next().unwrap().to_string();
        PaneNode {
            pane_id: id.to_string(),
            tab_id: format!("{workspace}:t1"),
            workspace_id: workspace,
            display_name: name.map(str::to_string),
            agent_status: status,
            focused: false,
        }
    }

    fn worktree(branch: &str, panes: Vec<PaneNode>) -> WorktreeNode {
        WorktreeNode {
            branch: Some(branch.to_string()),
            checkout_path: format!("/wt/{}", branch.replace('/', "-")),
            is_primary: branch == "main",
            open_workspace_id: panes.first().map(|p| p.workspace_id.clone()),
            panes,
        }
    }

    /// `me/app` on main (a working agent and a plain shell) and feat/login (idle), plus an
    /// unused fix/crash checkout; `me/site` on develop with a blocked agent.
    fn tree() -> Tree {
        Tree {
            repos: vec![
                RepoNode {
                    repo_key: "/src/app/.git".into(),
                    repo_root: "/src/app".into(),
                    display_name: "me/app".into(),
                    worktrees: vec![
                        worktree(
                            "main",
                            vec![
                                pane("w1:p1", Some("claude"), AgentStatus::Working),
                                pane("w1:p2", None, AgentStatus::Unknown),
                            ],
                        ),
                        worktree(
                            "feat/login",
                            vec![pane("w2:p1", Some("codex"), AgentStatus::Idle)],
                        ),
                        worktree("fix/crash", vec![]),
                    ],
                },
                RepoNode {
                    repo_key: "/src/site/.git".into(),
                    repo_root: "/src/site".into(),
                    display_name: "me/site".into(),
                    worktrees: vec![worktree(
                        "develop",
                        vec![pane("w3:p1", Some("claude"), AgentStatus::Blocked)],
                    )],
                },
            ],
            ungrouped: vec![pane("w9:p1", None, AgentStatus::Unknown)],
        }
    }

    /// The rows as `<indent><label>`, which is what the tree glyphs are drawn from.
    fn labels(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth as usize), r.label))
            .collect()
    }

    fn find<'a>(rows: &'a [Row], label: &str) -> &'a Row {
        rows.iter()
            .find(|r| r.label == label)
            .unwrap_or_else(|| panic!("no row labelled {label}"))
    }

    #[test]
    fn lays_the_tree_out_as_repo_worktree_pane_with_a_count_on_the_group() {
        assert_eq!(
            labels(&flatten(&tree(), &ViewOptions::default())),
            [
                "me/app (3)",
                "  main",
                "    claude",
                "    shell",
                "  feat/login",
                "    codex",
                "  fix/crash",
                "me/site (1)",
                "  develop",
                "    claude",
            ]
        );
    }

    #[test]
    fn a_blank_line_separates_each_group_but_not_the_first() {
        let rows = flatten(&tree(), &ViewOptions::default());
        let lines = display_lines(&rows);
        assert_eq!(lines[0], DisplayLine::Row(0), "no leading blank line");
        let spacers: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| **line == DisplayLine::Spacer)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(spacers.len(), 1, "one group boundary, one blank line");
        assert_eq!(
            lines[spacers[0] + 1],
            DisplayLine::Row(7),
            "me/site follows"
        );
    }

    #[test]
    fn the_meta_column_says_where_the_thing_is() {
        let rows = flatten(&tree(), &ViewOptions::default());
        assert_eq!(find(&rows, "main").meta, "/wt/main");
        assert_eq!(find(&rows, "feat/login").meta, "/wt/feat-login");
        assert_eq!(find(&rows, "claude").meta, "w1:p1");
        assert_eq!(find(&rows, "shell").meta, "w1:p2");
    }

    #[test]
    fn a_repository_leaves_its_path_to_the_checkout_below_it() {
        // The main checkout sits directly under it with the same path; printing both is
        // noise.
        let rows = flatten(&tree(), &ViewOptions::default());
        assert_eq!(find(&rows, "me/app (3)").meta, "");
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
    fn paths_under_the_home_directory_are_shortened() {
        let mut tree = tree();
        tree.repos[0].worktrees[0].checkout_path = "/home/me/Workspace/app".into();
        let options = ViewOptions {
            home: Some("/home/me".into()),
            ..Default::default()
        };
        assert_eq!(
            find(&flatten(&tree, &options), "main").meta,
            "~/Workspace/app"
        );
    }

    #[test]
    fn a_path_outside_the_home_directory_is_left_alone() {
        assert_eq!(abbreviate("/srv/app", Some("/home/me")), "/srv/app");
        // A sibling that merely shares the prefix must not be mangled.
        assert_eq!(
            abbreviate("/home/median/app", Some("/home/me")),
            "/home/median/app"
        );
        assert_eq!(abbreviate("/home/me", Some("/home/me")), "~");
        assert_eq!(abbreviate("/home/me/x", Some("/home/me/")), "~/x");
        assert_eq!(abbreviate("/home/me/x", None), "/home/me/x");
        assert_eq!(abbreviate("/home/me/x", Some("")), "/home/me/x");
    }

    #[test]
    fn a_parent_shows_the_most_urgent_state_underneath_it() {
        let rows = flatten(&tree(), &ViewOptions::default());
        // main holds a working agent and a plain shell; working wins over nothing.
        assert_eq!(find(&rows, "main").status, AgentStatus::Working);
        // The repository also holds an idle agent, but working is more urgent.
        assert_eq!(find(&rows, "me/app (3)").status, AgentStatus::Working);
        assert_eq!(find(&rows, "me/site (1)").status, AgentStatus::Blocked);
        assert_eq!(find(&rows, "fix/crash").status, AgentStatus::Unknown);
    }

    #[test]
    fn the_focused_pane_and_the_repository_holding_it_are_marked_as_current() {
        let mut tree = tree();
        tree.repos[1].worktrees[0].panes[0].focused = true;
        let rows = flatten(&tree, &ViewOptions::default());
        assert!(find(&rows, "me/site (1)").is_current);
        assert!(
            rows.iter()
                .find(|r| r.reference == RowRef::Pane(1, 0, 0))
                .unwrap()
                .is_current
        );
        assert!(!find(&rows, "me/app (3)").is_current);
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
        assert_eq!(labels(&rows), ["me/app (3)", "  feat/login", "    codex"]);
        assert!(find(&rows, "feat/login").matched);
        // Kept so the result has context, but it is not itself a result.
        assert!(!find(&rows, "me/app (3)").matched);
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
        assert_eq!(labels(&rows), ["me/site (1)", "  develop", "    claude"]);
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
            labels(&flatten(&tree(), &options)),
            ["me/app (3)", "  main", "    claude"]
        );
    }

    #[test]
    fn a_state_filter_that_matches_nothing_leaves_an_empty_list() {
        let options = ViewOptions {
            state_filter: Some(StateFilter::Done),
            show_ungrouped: true,
            ..Default::default()
        };
        assert!(flatten(&tree(), &options).is_empty());
    }

    #[test]
    fn panes_outside_a_repository_form_their_own_group_when_revealed() {
        let options = ViewOptions {
            show_ungrouped: true,
            ..Default::default()
        };
        let rows = flatten(&tree(), &options);
        let group = find(&rows, "not in any repository (1)");
        assert!(
            group.reference.is_group(),
            "it folds and spaces like a repo"
        );
        assert_eq!(rows.last().unwrap().label, "shell");
    }

    #[test]
    fn the_cursor_stops_only_where_there_is_somewhere_to_go() {
        let rows = flatten(&tree(), &ViewOptions::default());
        for row in &rows {
            let selectable = row.is_selectable();
            match row.reference {
                RowRef::Pane(..) | RowRef::Ungrouped(_) => {
                    assert!(selectable, "a pane is somewhere to go: {}", row.label)
                }
                RowRef::Worktree(..) => assert_eq!(
                    selectable, row.is_idle,
                    "only a checkout with nothing running: {}",
                    row.label
                ),
                RowRef::Repo(_) | RowRef::UngroupedRepo => {
                    assert!(!selectable, "a heading: {}", row.label)
                }
            }
        }
        // The fixture has all four kinds, so the loop above is not vacuous.
        assert!(rows.iter().any(|row| row.is_selectable()));
        assert!(rows.iter().any(|row| !row.is_selectable()));
    }

    #[test]
    fn every_repository_on_screen_has_something_selectable_under_it() {
        // Otherwise the arrow keys could reach a group and then have nowhere to go inside
        // it. A checkout either has panes, which are selectable, or has none, which makes
        // the checkout itself selectable.
        for options in [
            ViewOptions::default(),
            ViewOptions {
                query: "claude".into(),
                ..Default::default()
            },
            ViewOptions {
                state_filter: Some(StateFilter::Idle),
                ..Default::default()
            },
        ] {
            let rows = flatten(&tree(), &options);
            if rows.is_empty() {
                continue;
            }
            let mut group: Option<&str> = None;
            let mut seen = false;
            for row in &rows {
                if row.reference.is_group() {
                    if let Some(label) = group {
                        assert!(seen, "{label} has nothing the cursor can land on");
                    }
                    group = Some(&row.label);
                    seen = false;
                }
                seen |= row.is_selectable();
            }
            assert!(seen, "the last group has nothing the cursor can land on");
        }
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
        assert_eq!(flatten(&tree, &options)[0].label, "me/harken (0)");
    }

    #[test]
    fn cursor_movement_steps_over_everything_it_cannot_land_on_and_wraps() {
        let rows = flatten(
            &tree(),
            &ViewOptions {
                show_ungrouped: true,
                ..Default::default()
            },
        );
        let lines = display_lines(&rows);
        let stops: Vec<usize> = (0..lines.len())
            .filter(|index| selectable(&rows, &lines, *index))
            .collect();

        // The first line is a repository heading, so the cursor starts below it.
        assert!(!selectable(&rows, &lines, 0));
        assert_eq!(next_row(&rows, &lines, 0), Some(stops[0]));
        assert_eq!(
            previous_row(&rows, &lines, 0),
            stops.last().copied(),
            "and wraps to the last thing there is to go to"
        );

        // A blank line, the heading after it, and the checkout that already has panes are
        // all stepped over in one go.
        let spacer = lines
            .iter()
            .position(|line| *line == DisplayLine::Spacer)
            .unwrap();
        assert_eq!(
            next_row(&rows, &lines, spacer),
            stops.iter().find(|stop| **stop > spacer).copied()
        );
        assert_eq!(
            previous_row(&rows, &lines, spacer),
            stops.iter().rev().find(|stop| **stop < spacer).copied()
        );

        // A line that is already a stop is where it stays.
        assert_eq!(next_row(&rows, &lines, stops[1]), Some(stops[1]));
        assert_eq!(previous_row(&rows, &lines, stops[1]), Some(stops[1]));
    }

    #[test]
    fn cursor_movement_on_an_empty_list_has_no_answer_rather_than_panicking() {
        assert_eq!(next_row(&[], &[], 0), None);
        assert_eq!(previous_row(&[], &[], 0), None);
    }

    #[test]
    fn the_breadcrumb_carries_the_checkout_path_the_rows_no_longer_show() {
        let tree = tree();
        assert_eq!(
            detail(&tree, RowRef::Worktree(0, 1)),
            "me/app · feat/login · /wt/feat-login"
        );
        assert_eq!(
            detail(&tree, RowRef::Worktree(0, 0)),
            "me/app · main · main checkout · /wt/main"
        );
        assert_eq!(
            detail(&tree, RowRef::Pane(0, 0, 0)),
            "me/app · main · w1:p1 · working · /wt/main"
        );
        assert_eq!(
            detail(&tree, RowRef::Repo(0)),
            "me/app · 3 worktrees · 3 panes · /src/app"
        );
    }

    #[test]
    fn the_breadcrumb_is_empty_rather_than_panicking_on_a_stale_reference() {
        let tree = tree();
        assert_eq!(detail(&tree, RowRef::Repo(99)), "");
        assert_eq!(detail(&tree, RowRef::Worktree(0, 99)), "");
        assert_eq!(detail(&tree, RowRef::Pane(0, 0, 99)), "");
        assert_eq!(detail(&tree, RowRef::Ungrouped(99)), "");
    }
}
