//! Turning a herdr snapshot into the `repo -> worktree -> pane` tree the panes view shows.
//!
//! This is pure: the caller resolves where each pane lives and which worktrees each
//! repository has (via the ports), and hands the answers in.

use std::collections::HashMap;

use crate::domain::model::{normalize_path, PaneNode, Refs, RepoNode, Tree, WorktreeNode};
use crate::port::{GitRef, RefKind, Snapshot, Track, Worktree};

/// A repository the caller has identified, together with the worktrees herdr reported for it.
#[derive(Debug, Clone)]
pub struct RepoInput {
    pub repo_key: String,
    pub repo_root: String,
    /// `owner/repo` for a GitHub origin, otherwise the directory name.
    pub display_name: String,
    pub worktrees: Vec<Worktree>,
    /// The repository's refs, for what git says about each branch's upstream — or git's own
    /// words when the read failed. A checkout carries no marker then, which is the same
    /// thing it does for a branch with nothing to report, and the repository says so
    /// instead: see [`Refs`].
    pub refs: Result<Vec<GitRef>, String>,
}

/// What git said about the branch each checkout has out, by checkout.
///
/// Keyed on `%(worktreepath)` rather than on the branch name. git is answering "which
/// checkout has this ref", which is the question being asked here, where a name match would
/// have to guess. It is not enough on its own for a checkout with no branch out: git will
/// name such a path from a worktree entry that lost its directory, and
/// `a_ref_carrying_gone_can_name_a_path_whose_checkout_has_no_branch_out` in
/// `tests/git_adapter.rs` builds one carrying `[gone]`. `build` drops it for the rows herdr
/// listed; the row `build` makes for a pane herdr did not list keeps it, for want of
/// anything to decide on — herdr named no checkout there, so nothing here knows whether a
/// branch is out at it. That row's marker can therefore be a `[gone]` about a branch that
/// is not checked out, which is issue #49.
///
/// One map over every repository rather than a scan per checkout. The lookup for a checkout
/// herdr did not list used to reach into `repos` by an index that was only valid because
/// `nodes` happened to be built from it in order, and a `filter` added to that `map` would
/// have silently attached one repository's branch state to another's checkouts.
///
/// The repository is half the key, because a path is not enough to tell two answers apart.
/// A directory is the working tree of at most one repository, but git does not stop listing
/// a linked worktree whose directory was moved or deleted: the entry keeps naming the path
/// it had, and `git worktree prune` will not clear it once another repository has put a
/// worktree there. Two repositories then name one path with nothing to part them. Nothing
/// here asserts that on git's behalf: `tests/git_adapter.rs` builds it out of plain
/// `git worktree` commands and reads the walk back, in
/// `two_repositories_can_name_one_path_and_prune_will_not_part_them`.
///
/// A map keyed on the path alone takes whichever came last — so a stale `[gone]` landed on
/// a live checkout somewhere else, and it landed there even when that checkout's own
/// repository had said nothing at all, which is the one place [`Refs::Unreadable`] promises
/// no marker can come from. That is issue #31.
///
/// A repository that offers a second ref at one of its own paths is answered with nothing
/// rather than with either of them: the two disagree about which branch is at that path,
/// and a marker picked by whichever git listed first is not an answer. git makes that state
/// too, and `one_repository_can_name_one_path_from_two_refs` is where that is held rather
/// than said. What the second ref's `track` says does not come into it, and must not:
/// `None` there is a ref with nothing to report rather than the absence of a ref, which is
/// what [`GitRef::track`] means by it, and a second ref means a second local one — git puts
/// no checkout on a remote ref, and the adapter reads `%(worktreepath)` for both kinds
/// before it decides which kind it has.
///
/// Both halves of the key and both lookups go through `normalize_path`, so the match is one
/// between paths rather than between spellings. Each of those, and the local-only filter,
/// is held by a test rather than by this paragraph — one test per call, and
/// `a_repository_key_herdr_and_the_placement_spell_differently_is_one_repository` for the
/// pair on the repository half, which reads the same field at both ends and so goes on
/// matching itself when both are deleted at once. The spellings that actually have to be
/// reconciled arrive from two places, herdr's list and the pane's own `identify`, which is
/// what that test builds.
///
/// The two ways of losing one are not the same. Drop a `normalize_path` and the key misses:
/// the checkout draws no marker, which is also what a repository that said nothing about it
/// looks like — except on `build`'s own `repo_key`, where `by_key` misses instead and the
/// pane leaves the repository for `ungrouped` with its row. Drop the local-only filter and
/// the key hits: the entry collapses, per the `and_modify` below, and the marker goes the
/// same way for the opposite reason.
fn tracks(repos: &[RepoInput]) -> HashMap<(&str, &str), Track> {
    let mut found: HashMap<(&str, &str), Option<Track>> = HashMap::new();
    for repo in repos {
        let repo_key = normalize_path(&repo.repo_key);
        for git_ref in repo.refs.as_deref().unwrap_or_default() {
            if git_ref.kind != RefKind::Local {
                continue;
            }
            let Some(path) = git_ref.worktree_path.as_deref() else {
                continue;
            };
            found
                .entry((repo_key, normalize_path(path)))
                .and_modify(|held| *held = None)
                .or_insert(git_ref.track);
        }
    }
    found
        .into_iter()
        .filter_map(|(key, track)| Some((key, track?)))
        .collect()
}

/// Which repository and checkout a pane's working directory resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanePlacement {
    pub repo_key: String,
    pub checkout_path: String,
}

/// Build the tree. Panes with no placement are collected into `ungrouped`.
///
/// Nothing needs excluding: the picker runs as a popup, which herdr does not report as a
/// pane, so it never appears in its own list.
pub fn build(
    snapshot: &Snapshot,
    repos: &[RepoInput],
    placements: &HashMap<String, PanePlacement>,
) -> Tree {
    let tracks = tracks(repos);
    let mut nodes: Vec<RepoNode> = repos
        .iter()
        .map(|repo| RepoNode {
            repo_key: normalize_path(&repo.repo_key).to_string(),
            repo_root: normalize_path(&repo.repo_root).to_string(),
            display_name: repo.display_name.clone(),
            refs: match &repo.refs {
                Ok(_) => Refs::Read,
                Err(words) => Refs::Unreadable(words.clone()),
            },
            worktrees: repo
                .worktrees
                .iter()
                // A bare repository has no working tree, so no pane can ever sit in it.
                .filter(|worktree| !worktree.is_bare)
                .map(|worktree| {
                    let checkout_path = normalize_path(&worktree.path);
                    let branch = worktree.branch.clone().filter(|b| !b.is_empty());
                    // herdr says nothing is checked out here, so no ref of this
                    // repository's is about it. git can still name the path from an entry
                    // that lost its directory — see `tracks` — and that ref's `[gone]`
                    // would otherwise land on a row with no branch for it to be about,
                    // which `domain::sweep::judge` offers for deletion by default. The row
                    // built below for a pane herdr did not list keeps its track, and not
                    // because a branch is out there: herdr said nothing about that
                    // checkout at all, so there is nothing to make this call with.
                    let track = if branch.is_some() {
                        tracks
                            .get(&(normalize_path(&repo.repo_key), checkout_path))
                            .copied()
                    } else {
                        None
                    };
                    WorktreeNode {
                        branch,
                        checkout_path: checkout_path.to_string(),
                        is_primary: !worktree.is_linked_worktree,
                        open_workspace_id: worktree.open_workspace_id.clone(),
                        track,
                        panes: Vec::new(),
                    }
                })
                .collect(),
        })
        .collect();

    let mut by_key: HashMap<&str, usize> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        by_key.insert(node.repo_key.as_str(), index);
    }
    let by_key: HashMap<String, usize> = by_key
        .into_iter()
        .map(|(key, index)| (key.to_string(), index))
        .collect();

    let mut ungrouped = Vec::new();

    // Snapshot order is preserved within each worktree: herdr already lists panes in an
    // order that matches the layout, and re-sorting by id would put p10 before p9.
    for pane in &snapshot.panes {
        let node = PaneNode {
            pane_id: pane.pane_id.clone(),
            workspace_id: pane.workspace_id.clone(),
            tab_id: pane.tab_id.clone(),
            display_name: pane.display_name().map(str::to_string),
            agent_status: pane.agent_status,
            focused: pane.focused,
        };

        let Some(placement) = placements.get(&pane.pane_id) else {
            ungrouped.push(node);
            continue;
        };
        let Some(&index) = by_key.get(normalize_path(&placement.repo_key)) else {
            ungrouped.push(node);
            continue;
        };

        let checkout = normalize_path(&placement.checkout_path);
        // The same key `tracks` was built with, and provably the row's own repository:
        // `by_key` resolved this very string to `index` above, so it is `nodes[index]`'s
        // `repo_key`. The two lookup sites cannot disagree about who owns a checkout.
        let owner = normalize_path(&placement.repo_key);
        let repo = &mut nodes[index];
        match repo
            .worktrees
            .iter_mut()
            .find(|worktree| worktree.checkout_path == checkout)
        {
            Some(worktree) => worktree.panes.push(node),
            None => {
                // A checkout herdr's worktree list did not mention — for instance one added
                // with `git worktree add` outside herdr. Showing it is better than dropping
                // the pane into "ungrouped", where the user would not think to look.
                repo.worktrees.push(WorktreeNode {
                    branch: None,
                    checkout_path: checkout.to_string(),
                    is_primary: false,
                    open_workspace_id: Some(node.workspace_id.clone()),
                    // git knows about it even where herdr does not. Not the call made
                    // above: `branch: None` here is herdr never naming this checkout,
                    // which says nothing either way about what is out at it, so what git
                    // said stands — `[gone]` about a branch that is not checked out
                    // included, which is issue #49.
                    track: tracks.get(&(owner, checkout)).copied(),
                    panes: vec![node],
                });
            }
        }
    }

    for repo in &mut nodes {
        // The main checkout first, then linked worktrees alphabetically. This keeps the
        // repository's "home" row in a stable place as worktrees come and go.
        repo.worktrees.sort_by(|a, b| {
            b.is_primary
                .cmp(&a.is_primary)
                .then_with(|| a.label().cmp(b.label()))
        });
    }
    nodes.sort_by(|a, b| a.display_name.cmp(&b.display_name));

    Tree {
        repos: nodes,
        ungrouped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::AgentStatus;
    use serde_json::json;
    use std::num::NonZeroU32;

    /// Build a snapshot from the wire shape herdr actually sends, so these tests also
    /// exercise the deserializers.
    fn snapshot(panes: serde_json::Value) -> Snapshot {
        serde_json::from_value(json!({
            "version": "0.7.4",
            "protocol": 16,
            "workspaces": [],
            "tabs": [],
            "panes": panes,
        }))
        .expect("snapshot fixture should deserialize")
    }

    fn pane(pane_id: &str, agent: Option<&str>) -> serde_json::Value {
        let (workspace_id, _) = pane_id.split_once(':').unwrap();
        json!({
            "pane_id": pane_id,
            "tab_id": format!("{workspace_id}:t1"),
            "workspace_id": workspace_id,
            "terminal_id": format!("term_{pane_id}"),
            "focused": false,
            "agent": agent,
            "agent_status": if agent.is_some() { "idle" } else { "unknown" },
        })
    }

    fn worktree(branch: &str, path: &str, linked: bool) -> Worktree {
        serde_json::from_value(json!({
            "branch": branch,
            "path": path,
            "label": branch,
            "is_bare": false,
            "is_detached": false,
            "is_linked_worktree": linked,
            "is_prunable": false,
        }))
        .expect("worktree fixture should deserialize")
    }

    fn repo(display_name: &str, root: &str, worktrees: Vec<Worktree>) -> RepoInput {
        RepoInput {
            // Matches herdr: repo_key has no trailing slash, repo_root may have one.
            repo_key: format!("{}/.git", normalize_path(root)),
            repo_root: root.to_string(),
            display_name: display_name.to_string(),
            worktrees,
            refs: Ok(Vec::new()),
        }
    }

    fn remote_ref(name: &str, worktree_path: Option<&str>, track: Option<Track>) -> GitRef {
        GitRef {
            kind: RefKind::Remote,
            ..local_ref(name, worktree_path, track)
        }
    }

    fn local_ref(name: &str, worktree_path: Option<&str>, track: Option<Track>) -> GitRef {
        GitRef {
            name: name.to_string(),
            kind: RefKind::Local,
            committed_at: None,
            subject: None,
            upstream: None,
            track,
            worktree_path: worktree_path.map(str::to_string),
        }
    }

    #[test]
    fn a_checkout_with_no_branch_out_draws_on_no_ref() {
        // git names a path from a worktree entry that lost its directory, and the checkout
        // sitting at that path can have nothing checked out —
        // `a_ref_carrying_gone_can_name_a_path_whose_checkout_has_no_branch_out` in
        // `tests/git_adapter.rs` builds exactly that. The row has no branch for a `[gone]`
        // to be about, and `domain::sweep::judge` offers a clean `gone` row for deletion by
        // default, so the marker must not reach it.
        let shared = "/wt/shared";
        let mut app = repo("me/app", "/src/app", vec![]);
        app.worktrees = vec![serde_json::from_value(json!({
            "branch": "",
            "path": shared,
            "label": "shared",
            "is_bare": false,
            "is_detached": true,
            "is_linked_worktree": true,
            "is_prunable": false,
        }))
        .expect("worktree fixture should deserialize")];
        app.refs = Ok(vec![local_ref(
            "chore/deps",
            Some(shared),
            Some(Track::Gone),
        )]);

        let row = &build(&snapshot(json!([])), &[app], &HashMap::new()).repos[0].worktrees[0];
        assert_eq!(row.branch, None, "herdr says nothing is checked out there");
        assert_eq!(
            row.track, None,
            "so no ref of this repository's is about it"
        );
    }

    #[test]
    fn what_a_branchless_row_draws_turns_on_whether_herdr_listed_it() {
        // The two rows `build` makes, over one repository's identical git facts: a stale
        // registration goes on naming `/wt/shared` for `chore/deps`, whose upstream was
        // deleted, and the checkout sitting there has nothing out. The row herdr listed is
        // refused the marker, above. The row `build` makes for a pane herdr never
        // mentioned keeps it, because at that site there is nothing to make the call
        // with — not because a branch is out there. Both carry `branch: None` and nothing
        // afterwards can tell them apart, so the second draws `gone` beside a directory
        // name about a branch it never names. That is issue #49; pinned here so it stays a
        // difference somebody measured rather than one nobody looked at.
        let shared = "/wt/shared";
        let stale = || local_ref("chore/deps", Some(shared), Some(Track::Gone));

        let mut listed = repo("me/app", "/src/app", vec![]);
        listed.worktrees = vec![serde_json::from_value(json!({
            "branch": "",
            "path": shared,
            "label": "shared",
            "is_bare": false,
            "is_detached": true,
            "is_linked_worktree": true,
            "is_prunable": false,
        }))
        .expect("worktree fixture should deserialize")];
        listed.refs = Ok(vec![stale()]);
        let listed = &build(&snapshot(json!([])), &[listed], &HashMap::new()).repos[0].worktrees[0];

        let mut unlisted = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );
        unlisted.refs = Ok(vec![stale()]);
        let tree = build(
            &snapshot(json!([pane("w1:p1", None)])),
            &[unlisted],
            &placements(&[("w1:p1", "/src/app/.git", shared)]),
        );
        let unlisted = tree.repos[0]
            .worktrees
            .iter()
            .find(|worktree| worktree.checkout_path == shared)
            .expect("the row build made for the pane");

        assert_eq!(
            (listed.branch.as_deref(), unlisted.branch.as_deref()),
            (None, None),
            "neither row names a branch"
        );
        assert_eq!(
            (listed.track, unlisted.track),
            (None, Some(Track::Gone)),
            "and only the one herdr spoke about is refused the marker"
        );
    }

    #[test]
    fn a_track_is_read_from_the_repository_that_owns_the_checkout() {
        // Two repositories naming one path, which `tests/git_adapter.rs` shows git does.
        // Pooled into one map the second one wins, and which that is depends on the order
        // the repositories were listed in.
        let shared = "/wt/shared";
        let mut stale = repo("me/old", "/src/old", vec![]);
        stale.refs = Ok(vec![local_ref("old", Some(shared), Some(Track::Gone))]);
        let mut live = repo(
            "me/app",
            "/src/app",
            vec![worktree("feat/login", shared, true)],
        );
        let ahead = Track::Ahead(NonZeroU32::new(1).unwrap());
        live.refs = Ok(vec![local_ref("feat/login", Some(shared), Some(ahead))]);

        for order in [
            vec![stale.clone(), live.clone()],
            vec![live.clone(), stale.clone()],
        ] {
            let tree = build(&snapshot(json!([])), &order, &HashMap::new());
            let app = tree
                .repos
                .iter()
                .find(|repo| repo.display_name == "me/app")
                .expect("the live repository");
            assert_eq!(
                app.worktrees[0].track,
                Some(ahead),
                "the repository that owns the checkout is what says where it stands"
            );
        }
    }

    #[test]
    fn two_tracked_refs_at_one_of_a_repository_s_own_paths_answer_nothing() {
        // git does make this — `one_repository_can_name_one_path_from_two_refs` builds it —
        // and taking the last would be a marker chosen by iteration order. The honest
        // answer is the one a checkout with no ref of its own gets.
        let shared = "/wt/shared";
        let mut app = repo(
            "me/app",
            "/src/app",
            vec![worktree("feat/login", shared, true)],
        );
        app.refs = Ok(vec![
            local_ref("feat/login", Some(shared), Some(Track::Gone)),
            local_ref(
                "chore/deps",
                Some(shared),
                Some(Track::Ahead(NonZeroU32::new(1).unwrap())),
            ),
        ]);

        let tree = build(&snapshot(json!([])), &[app], &HashMap::new());
        assert_eq!(tree.repos[0].worktrees[0].track, None);
    }

    #[test]
    fn a_second_ref_counts_even_where_git_had_nothing_to_report_about_it() {
        // `track: None` is a ref with nothing to report, not the absence of a ref.
        // [`GitRef::track`] arrives at it two ways — level with whatever it was measured
        // against, and having neither an upstream nor a push destination to be measured
        // against — and this fixture's ref has neither, `local_ref` leaving `upstream`
        // unset. Having no upstream is not one of them on its own: git measures such a
        // branch against where it would push, which
        // `a_branch_with_no_upstream_is_still_measured_against_where_it_would_push` in
        // `tests/git_adapter.rs` reads back as a marker rather than as nothing. It
        // contradicts the `[gone]` beside it about which branch is
        // at that path exactly as a marked ref would. Skipping it would let the `[gone]` in
        // unopposed, and `domain::sweep::judge` offers a `gone` row for deletion by default.
        let shared = "/wt/shared";
        for order in [
            vec![("feat/login", None), ("chore/deps", Some(Track::Gone))],
            vec![("chore/deps", Some(Track::Gone)), ("feat/login", None)],
        ] {
            let mut app = repo(
                "me/app",
                "/src/app",
                vec![worktree("feat/login", shared, true)],
            );
            app.refs = Ok(order
                .iter()
                .map(|(name, track)| local_ref(name, Some(shared), *track))
                .collect());

            let tree = build(&snapshot(json!([])), &[app], &HashMap::new());
            assert_eq!(
                tree.repos[0].worktrees[0].track, None,
                "listed as {order:?}"
            );
        }
    }

    #[test]
    fn a_pane_s_own_row_is_read_from_the_repository_the_pane_is_in() {
        // The other lookup: a pane in a checkout herdr's worktree list did not mention, so
        // the row is made here rather than from the list. It has to draw on the repository
        // the pane is in and not on whichever repository happens to name that path — this
        // is issue #31 on the row `build` synthesizes itself.
        let shared = "/wt/shared";
        let mut stale = repo("me/old", "/src/old", vec![]);
        stale.refs = Ok(vec![local_ref("old", Some(shared), Some(Track::Gone))]);
        let app = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );

        let tree = build(
            &snapshot(json!([pane("w1:p1", None)])),
            &[stale, app],
            &placements(&[("w1:p1", "/src/app/.git", shared)]),
        );
        let app = tree
            .repos
            .iter()
            .find(|repo| repo.display_name == "me/app")
            .expect("the repository the pane is in");
        let synthesized = app
            .worktrees
            .iter()
            .find(|worktree| worktree.checkout_path == shared)
            .expect("the row build made for the pane");
        assert_eq!(
            synthesized.track, None,
            "me/old's ref is not me/app's answer"
        );
    }

    #[test]
    fn a_repository_whose_refs_were_not_read_has_no_track_to_draw_from() {
        // What `Refs::Unreadable` promises: every checkout under it is missing its markers.
        // A stale entry in another repository is what used to break that promise, and it
        // broke it where it matters most — `domain::sweep::judge` reads `gone` before it
        // reads `refs`, so the row was offered for deletion by default while the prompt
        // line said the refs could not be read.
        let shared = "/wt/shared";
        let mut stale = repo("me/old", "/src/old", vec![]);
        stale.refs = Ok(vec![local_ref("old", Some(shared), Some(Track::Gone))]);
        let mut unread = repo(
            "me/app",
            "/src/app",
            vec![worktree("feat/login", shared, true)],
        );
        unread.refs = Err("fatal: bad ref (`git for-each-ref …`)".to_string());

        let tree = build(&snapshot(json!([])), &[stale, unread], &HashMap::new());
        let app = tree
            .repos
            .iter()
            .find(|repo| repo.display_name == "me/app")
            .expect("the unreadable repository");
        assert_eq!(app.worktrees[0].track, None, "refs: {:?}", app.refs);
    }

    #[test]
    fn a_remote_ref_at_a_checkout_is_not_a_second_ref_of_the_repositorys() {
        // `tracks` reads local refs and nothing else, and after this the filter is what
        // stands between a non-local ref carrying a `%(worktreepath)` and the whole
        // checkout losing its marker: a second ref at a path is answered with nothing, so
        // one counted in error takes the marker away rather than merely overwriting it.
        // Why a ref git puts no checkout on can reach here carrying one is in `tracks`'
        // own paragraph. `app::dump` holds the same filter over the same refs and pins it
        // in `a_ref_git_lists_no_checkout_for_does_not_answer_for_one`; this is the other
        // one, and neither stood for the other.
        let shared = "/wt/shared";
        let mut app = repo(
            "me/app",
            "/src/app",
            vec![worktree("feat/login", shared, true)],
        );
        app.refs = Ok(vec![
            local_ref("feat/login", Some(shared), Some(Track::Gone)),
            remote_ref("origin/feat/login", Some(shared), None),
        ]);

        let tree = build(&snapshot(json!([])), &[app], &HashMap::new());
        assert_eq!(
            tree.repos[0].worktrees[0].track,
            Some(Track::Gone),
            "a remote ref is not a second ref at this checkout"
        );
    }

    #[test]
    fn a_repository_key_spelled_with_a_trailing_slash_still_names_one_repository() {
        // The repository half of the key, both where it is built and where it is read.
        // Spelled two ways it would be two repositories, and a checkout would lose every
        // marker with nothing said — the same silence a repository with no refs at all
        // makes.
        let shared = "/wt/shared";
        let mut app = repo(
            "me/app",
            "/src/app",
            vec![worktree("feat/login", shared, true)],
        );
        app.repo_key = "/src/app/.git/".to_string();
        app.refs = Ok(vec![local_ref(
            "feat/login",
            Some(shared),
            Some(Track::Gone),
        )]);

        let tree = build(&snapshot(json!([])), &[app], &HashMap::new());
        assert_eq!(tree.repos[0].worktrees[0].track, Some(Track::Gone));
    }

    #[test]
    fn a_checkout_spelled_one_way_by_git_and_another_by_herdr_is_one_checkout() {
        // The path half of the key, and the path the row looks itself up by. The two
        // spellings arrive from different places — `%(worktreepath)` from git, `path` from
        // herdr — so either of them can be the one carrying the slash.
        for (herdr_says, git_says) in [("/wt/shared/", "/wt/shared"), ("/wt/shared", "/wt/shared/")]
        {
            let mut app = repo(
                "me/app",
                "/src/app",
                vec![worktree("feat/login", herdr_says, true)],
            );
            app.refs = Ok(vec![local_ref(
                "feat/login",
                Some(git_says),
                Some(Track::Gone),
            )]);

            let tree = build(&snapshot(json!([])), &[app], &HashMap::new());
            assert_eq!(
                tree.repos[0].worktrees[0].track,
                Some(Track::Gone),
                "herdr said {herdr_says}, git said {git_says}"
            );
        }
    }

    #[test]
    fn a_panes_repository_key_spelled_with_a_trailing_slash_still_reaches_its_refs() {
        // The same repository half at the other lookup, on the row `build` makes itself.
        // `by_key` normalizes separately, so the pane is placed either way and only the
        // marker goes missing.
        let shared = "/wt/shared";
        let mut app = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );
        app.refs = Ok(vec![local_ref(
            "feat/login",
            Some(shared),
            Some(Track::Gone),
        )]);

        let tree = build(
            &snapshot(json!([pane("w1:p1", None)])),
            &[app],
            &placements(&[("w1:p1", "/src/app/.git/", shared)]),
        );
        let synthesized = tree.repos[0]
            .worktrees
            .iter()
            .find(|worktree| worktree.checkout_path == shared)
            .expect("the row build made for the pane");
        assert_eq!(synthesized.track, Some(Track::Gone));
    }

    #[test]
    fn a_repository_key_herdr_and_the_placement_spell_differently_is_one_repository() {
        // The other direction of the slash, and the one the tests above could not see: they
        // spell `RepoInput::repo_key` once and read it back from the same field, so the two
        // `normalize_path` calls on the repository half could be deleted *together* with the
        // whole suite green. The spellings that have to be reconciled arrive from two
        // places — herdr's worktree list and the pane's own `identify` — and `app::collect`
        // normalizing both before either struct exists is what makes them agree today.
        //
        // Two things go when the match is a match between spellings. `by_key` is built from
        // `RepoInput::repo_key` and looked up by the placement's, so the pane leaves the
        // repository for `ungrouped` and its row with it; and `tracks`' key is built from
        // one and read by the other, so the row that is left draws no marker.
        let shared = "/wt/shared";
        let mut app = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );
        app.repo_key = "/src/app/.git/".to_string();
        app.refs = Ok(vec![local_ref(
            "feat/login",
            Some(shared),
            Some(Track::Gone),
        )]);

        let tree = build(
            &snapshot(json!([pane("w1:p1", None)])),
            &[app],
            &placements(&[("w1:p1", "/src/app/.git", shared)]),
        );
        assert!(tree.ungrouped.is_empty(), "the pane must not be lost");
        let synthesized = tree.repos[0]
            .worktrees
            .iter()
            .find(|worktree| worktree.checkout_path == shared)
            .expect("the row build made for the pane");
        assert_eq!(synthesized.track, Some(Track::Gone));
    }

    #[test]
    fn a_panes_own_row_is_read_from_the_checkout_the_pane_is_in() {
        // The checkout half of the key at the synthesized site. The row herdr listed is
        // guarded on both halves — `a_checkout_carries_what_git_said_about_the_branch_it_
        // has_out` holds the checkout half there — and this one was guarded on the
        // repository half alone, so a stale `[gone]` naming some other path of the same
        // repository could land here. That is issue #31's shape moved inside one
        // repository, and `domain::sweep::judge` offers a clean `gone` row for deletion by
        // default.
        let mut app = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );
        app.refs = Ok(vec![local_ref(
            "feat/login",
            Some("/wt/login"),
            Some(Track::Gone),
        )]);

        let tree = build(
            &snapshot(json!([pane("w1:p1", None)])),
            &[app],
            &placements(&[("w1:p1", "/src/app/.git", "/wt/shared")]),
        );
        let synthesized = tree.repos[0]
            .worktrees
            .iter()
            .find(|worktree| worktree.checkout_path == "/wt/shared")
            .expect("the row build made for the pane");
        assert_eq!(
            synthesized.track, None,
            "git names no ref at the checkout this pane is in"
        );
    }

    #[test]
    fn a_checkout_carries_what_git_said_about_the_branch_it_has_out() {
        let mut input = repo(
            "me/app",
            "/src/app",
            vec![
                worktree("main", "/src/app", false),
                worktree("feat/login", "/wt/feat-login", true),
            ],
        );
        input.refs = Ok(vec![
            local_ref("main", Some("/src/app"), None),
            local_ref(
                "feat/login",
                Some("/wt/feat-login"),
                Some(Track::Diverged {
                    ahead: NonZeroU32::new(2).unwrap(),
                    behind: NonZeroU32::new(1).unwrap(),
                }),
            ),
        ]);

        let tree = build(&snapshot(json!([])), &[input], &HashMap::new());
        let worktrees = &tree.repos[0].worktrees;
        assert_eq!(worktrees[0].track, None);
        assert_eq!(
            worktrees[1].track,
            Some(Track::Diverged {
                ahead: NonZeroU32::new(2).unwrap(),
                behind: NonZeroU32::new(1).unwrap()
            })
        );
    }

    #[test]
    fn a_repository_whose_refs_could_not_be_read_says_so_and_marks_nothing() {
        // What the read failing used to do — nothing — is still what the rows do: no marker
        // beats a wrong one. What is new is that the repository carries git's words, so the
        // prompt line can say which it was. The other repository is untouched either way.
        let mut app = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );
        app.refs = Err("fatal: bad ref (`git for-each-ref …`)".to_string());
        let mut site = repo(
            "me/site",
            "/src/site",
            vec![worktree("main", "/src/site", false)],
        );
        site.refs = Ok(vec![local_ref(
            "main",
            Some("/src/site"),
            Some(Track::Gone),
        )]);

        let tree = build(&snapshot(json!([])), &[app, site], &HashMap::new());
        assert_eq!(
            tree.repos[0].refs,
            Refs::Unreadable("fatal: bad ref (`git for-each-ref …`)".to_string())
        );
        assert_eq!(tree.repos[0].worktrees[0].track, None);
        assert_eq!(tree.repos[1].refs, Refs::Read);
        assert_eq!(tree.repos[1].worktrees[0].track, Some(Track::Gone));
    }

    #[test]
    fn a_checkout_herdr_did_not_list_still_gets_what_git_said_about_it() {
        // The path the index-based lookup used to take, and the reason it was replaced: it
        // reached into `repos` by an index that was only valid because `nodes` happened to
        // be built from it in order.
        let mut input = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );
        input.refs = Ok(vec![
            local_ref("main", Some("/src/app"), None),
            local_ref("manual", Some("/elsewhere/manual"), Some(Track::Gone)),
        ]);
        let tree = build(
            &snapshot(json!([pane("w1:p1", None)])),
            &[input],
            &placements(&[("w1:p1", "/src/app/.git", "/elsewhere/manual")]),
        );
        let synthesized = tree.repos[0]
            .worktrees
            .iter()
            .find(|worktree| worktree.checkout_path == "/elsewhere/manual")
            .expect("herdr did not list it, so the pane's own cwd put it there");
        assert_eq!(synthesized.track, Some(Track::Gone));
    }

    #[test]
    fn one_repositorys_branch_state_never_lands_on_anothers_checkout() {
        // Two repositories, a branch of the same name in each, and no order between them
        // that the lookup is allowed to depend on. Checkout paths are absolute, so they are
        // what tells the two apart.
        let mut app = repo(
            "me/app",
            "/src/app",
            vec![worktree("main", "/src/app", false)],
        );
        app.refs = Ok(vec![local_ref("main", Some("/src/app"), Some(Track::Gone))]);
        let mut site = repo(
            "me/site",
            "/src/site",
            vec![worktree("main", "/src/site", false)],
        );
        site.refs = Ok(vec![local_ref("main", Some("/src/site"), None)]);

        let tree = build(&snapshot(json!([])), &[app, site], &HashMap::new());
        assert_eq!(tree.repos[0].worktrees[0].track, Some(Track::Gone));
        assert_eq!(tree.repos[1].worktrees[0].track, None, "not the other's");
    }

    #[test]
    fn the_branch_is_matched_by_the_checkout_git_says_has_it() {
        // Not by name. A ref that is not checked out anywhere says nothing about a checkout
        // that merely shares its name, and a detached checkout has nothing pointing at it —
        // which is exactly right: no marker rather than the wrong one.
        let mut input = repo(
            "me/app",
            "/src/app",
            vec![
                worktree("main", "/src/app", false),
                worktree("", "/wt/detached", true),
            ],
        );
        input.refs = Ok(vec![
            local_ref("main", Some("/src/app"), Some(Track::Gone)),
            local_ref("feat/login", None, Some(Track::Gone)),
        ]);

        let tree = build(&snapshot(json!([])), &[input], &HashMap::new());
        let worktrees = &tree.repos[0].worktrees;
        assert_eq!(worktrees[0].track, Some(Track::Gone));
        assert_eq!(worktrees[1].branch, None, "detached");
        assert_eq!(worktrees[1].track, None);
    }

    fn placements(pairs: &[(&str, &str, &str)]) -> HashMap<String, PanePlacement> {
        pairs
            .iter()
            .map(|(pane_id, repo_key, checkout)| {
                (
                    (*pane_id).to_string(),
                    PanePlacement {
                        repo_key: (*repo_key).to_string(),
                        checkout_path: (*checkout).to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn groups_panes_under_the_worktree_they_sit_in() {
        let tree = build(
            &snapshot(json!([pane("w1:p1", Some("claude")), pane("w2:p1", None)])),
            &[repo(
                "me/app",
                "/src/app",
                vec![
                    worktree("main", "/src/app", false),
                    worktree("feat/login", "/wt/app/feat-login", true),
                ],
            )],
            &placements(&[
                ("w1:p1", "/src/app/.git", "/src/app"),
                ("w2:p1", "/src/app/.git", "/wt/app/feat-login"),
            ]),
        );

        assert_eq!(tree.repos.len(), 1);
        let repo = &tree.repos[0];
        assert_eq!(repo.display_name, "me/app");
        assert_eq!(repo.worktrees.len(), 2);
        assert_eq!(repo.worktrees[0].label(), "main");
        assert_eq!(repo.worktrees[0].panes[0].pane_id, "w1:p1");
        assert_eq!(
            repo.worktrees[0].panes[0].display_name.as_deref(),
            Some("claude")
        );
        assert_eq!(repo.worktrees[1].label(), "feat/login");
        assert_eq!(repo.worktrees[1].panes[0].pane_id, "w2:p1");
        assert!(tree.ungrouped.is_empty());
    }

    #[test]
    fn keeps_worktrees_that_have_no_pane_as_openable_rows() {
        let tree = build(
            &snapshot(json!([pane("w1:p1", Some("claude"))])),
            &[repo(
                "me/app",
                "/src/app",
                vec![
                    worktree("main", "/src/app", false),
                    worktree("fix/crash", "/wt/app/fix-crash", true),
                ],
            )],
            &placements(&[("w1:p1", "/src/app/.git", "/src/app")]),
        );

        let idle = &tree.repos[0].worktrees[1];
        assert_eq!(idle.label(), "fix/crash");
        assert!(
            idle.is_idle(),
            "a checkout with no pane should read as idle"
        );
    }

    #[test]
    fn puts_panes_outside_any_repository_into_ungrouped() {
        let tree = build(
            &snapshot(json!([pane("w1:p1", None), pane("w9:p1", None)])),
            &[repo(
                "me/app",
                "/src/app",
                vec![worktree("main", "/src/app", false)],
            )],
            &placements(&[("w1:p1", "/src/app/.git", "/src/app")]),
        );

        assert_eq!(tree.ungrouped.len(), 1);
        assert_eq!(tree.ungrouped[0].pane_id, "w9:p1");
    }

    #[test]
    fn matches_a_pane_to_its_worktree_despite_herdrs_trailing_slashes() {
        let tree = build(
            &snapshot(json!([pane("w1:p1", None)])),
            &[repo(
                "me/app",
                "/src/app/",
                vec![worktree("main", "/src/app", false)],
            )],
            // herdr reports repo_root with a trailing slash and worktree.path without one.
            &placements(&[("w1:p1", "/src/app/.git", "/src/app/")]),
        );

        assert_eq!(tree.repos[0].worktrees[0].panes.len(), 1);
        assert!(tree.ungrouped.is_empty());
    }

    #[test]
    fn synthesizes_a_row_for_a_checkout_herdr_did_not_list() {
        let tree = build(
            &snapshot(json!([pane("w4:p1", Some("codex"))])),
            &[repo(
                "me/app",
                "/src/app",
                vec![worktree("main", "/src/app", false)],
            )],
            &placements(&[("w4:p1", "/src/app/.git", "/elsewhere/manual-worktree")]),
        );

        assert!(tree.ungrouped.is_empty(), "the pane must not be lost");
        let synthesized = tree.repos[0]
            .worktrees
            .iter()
            .find(|w| w.checkout_path == "/elsewhere/manual-worktree")
            .expect("the unknown checkout should appear under its repository");
        assert_eq!(synthesized.panes[0].pane_id, "w4:p1");
        assert_eq!(synthesized.label(), "manual-worktree");
    }

    #[test]
    fn orders_repositories_alphabetically_and_the_main_checkout_first() {
        let tree = build(
            &snapshot(json!([])),
            &[
                repo(
                    "me/zeta",
                    "/src/zeta",
                    vec![worktree("main", "/src/zeta", false)],
                ),
                repo(
                    "me/alpha",
                    "/src/alpha",
                    vec![
                        worktree("feat/b", "/wt/alpha/b", true),
                        worktree("feat/a", "/wt/alpha/a", true),
                        worktree("main", "/src/alpha", false),
                    ],
                ),
            ],
            &placements(&[]),
        );

        let names: Vec<_> = tree.repos.iter().map(|r| r.display_name.as_str()).collect();
        assert_eq!(names, ["me/alpha", "me/zeta"]);
        let labels: Vec<_> = tree.repos[0].worktrees.iter().map(|w| w.label()).collect();
        assert_eq!(labels, ["main", "feat/a", "feat/b"]);
    }

    #[test]
    fn preserves_the_snapshot_order_of_panes_within_a_worktree() {
        // Sorting by pane id would put p10 before p9; herdr's order is the layout order.
        let tree = build(
            &snapshot(json!([
                pane("w1:p9", None),
                pane("w1:p10", None),
                pane("w1:p1", None)
            ])),
            &[repo(
                "me/app",
                "/src/app",
                vec![worktree("main", "/src/app", false)],
            )],
            &placements(&[
                ("w1:p9", "/src/app/.git", "/src/app"),
                ("w1:p10", "/src/app/.git", "/src/app"),
                ("w1:p1", "/src/app/.git", "/src/app"),
            ]),
        );

        let ids: Vec<_> = tree.repos[0].worktrees[0]
            .panes
            .iter()
            .map(|p| p.pane_id.as_str())
            .collect();
        assert_eq!(ids, ["w1:p9", "w1:p10", "w1:p1"]);
    }

    #[test]
    fn skips_bare_repositories_which_can_never_hold_a_pane() {
        let mut bare = worktree("main", "/src/app.git", false);
        bare.is_bare = true;
        let tree = build(
            &snapshot(json!([])),
            &[repo("me/app", "/src/app", vec![bare])],
            &placements(&[]),
        );
        assert!(tree.repos[0].worktrees.is_empty());
    }

    #[test]
    fn finds_which_repository_and_checkout_a_pane_belongs_to() {
        let tree = build(
            &snapshot(json!([pane("w2:p1", Some("claude"))])),
            &[repo(
                "me/app",
                "/src/app",
                vec![
                    worktree("main", "/src/app", false),
                    worktree("feat/login", "/wt/app/feat-login", true),
                ],
            )],
            &placements(&[("w2:p1", "/src/app/.git", "/wt/app/feat-login")]),
        );

        let (repo, worktree, pane) = tree.find_pane("w2:p1").expect("pane should be found");
        assert_eq!(repo.display_name, "me/app");
        assert_eq!(worktree.label(), "feat/login");
        assert_eq!(pane.agent_status, AgentStatus::Idle);
        assert!(tree.find_pane("w9:p9").is_none());
    }
}
