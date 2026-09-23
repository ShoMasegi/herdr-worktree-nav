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
    assert_eq!(worktrees[0].position, Position::Said(None));
    assert_eq!(
        worktrees[1].position,
        Position::Said(Some(Track::Diverged {
            ahead: NonZeroU32::new(2).unwrap(),
            behind: NonZeroU32::new(1).unwrap()
        }))
    );
}

#[test]
fn a_repository_whose_refs_could_not_be_read_says_so_and_marks_nothing() {
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
    assert_eq!(tree.repos[0].worktrees[0].position, Position::NotSaid);
    assert_eq!(tree.repos[1].refs, Refs::Read);
    assert_eq!(
        tree.repos[1].worktrees[0].position,
        Position::Said(Some(Track::Gone))
    );
}

#[test]
fn a_checkout_herdr_did_not_list_still_gets_what_git_said_about_it() {
    // The path an index-based lookup gets wrong: reaching into `repos` by an index is
    // only valid while `nodes` happens to be built from it in order, and nothing makes
    // that so.
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
    assert_eq!(synthesized.position, Position::Said(Some(Track::Gone)));
}

#[test]
fn one_repositorys_branch_state_never_lands_on_anothers_checkout() {
    // Checkout paths are absolute, so they are what tells two same-named branches apart.
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
    assert_eq!(
        tree.repos[0].worktrees[0].position,
        Position::Said(Some(Track::Gone))
    );
    assert_eq!(
        tree.repos[1].worktrees[0].position,
        Position::Said(None),
        "not the other's"
    );
}

#[test]
fn the_branch_is_matched_by_the_checkout_git_says_has_it() {
    // A ref that is checked out nowhere says nothing about a checkout that merely shares
    // its name, and a detached checkout has nothing pointing at it.
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
    assert_eq!(worktrees[0].position, Position::Said(Some(Track::Gone)));
    assert_eq!(worktrees[1].branch, Branch::NothingOut, "detached");
    assert_eq!(worktrees[1].position, Position::NotSaid);
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

#[test]
fn a_track_is_read_from_the_repository_that_owns_the_checkout() {
    // Two repositories naming one path, which `tests/git_adapter.rs` shows git does.
    // Pooled into one map, which of them answers depends on the order they came in.
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
            app.worktrees[0].position,
            Position::Said(Some(ahead)),
            "the repository that owns the checkout is what says where it stands"
        );
    }
}

#[test]
fn two_tracked_refs_at_one_of_a_repository_s_own_paths_are_contested() {
    // git does make this: `one_repository_can_name_one_path_from_two_refs` builds it.
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
    assert_eq!(tree.repos[0].worktrees[0].position, Position::Contested);
}

#[test]
fn a_second_ref_counts_even_where_git_had_nothing_to_report_about_it() {
    // `track: None` is a ref with nothing to report, not the absence of a ref. It
    // contradicts the `[gone]` beside it about which branch is at that path exactly as
    // a marked ref would. Skipping it would let the `[gone]` in unopposed, and
    // `domain::sweep::judge` offers a clean `gone` row for deletion by default —
    // `a_clean_checkout_whose_upstream_is_gone_is_offered_with_its_reason`.
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
            tree.repos[0].worktrees[0].position,
            Position::Contested,
            "listed as {order:?}"
        );
    }
}

#[test]
fn a_pane_s_own_row_is_read_from_the_repository_the_pane_is_in() {
    // Issue #31 on the row `build` synthesizes itself: it has to draw on the repository
    // the pane is in, not on whichever repository happens to name that path.
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
        synthesized.position,
        Position::NotSaid,
        "me/old's ref is not me/app's answer"
    );
}

#[test]
fn a_repository_whose_refs_were_not_read_has_no_track_to_draw_from() {
    // What `Refs::Unreadable` promises: every checkout under it is missing its markers.
    // A stale entry in another repository is what breaks that promise, and it breaks
    // it where it matters most — `domain::sweep::judge` reads `gone` before it reads
    // `refs`, so the row is offered for deletion by default while the prompt line says
    // the refs could not be read.
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
    assert_eq!(
        app.worktrees[0].position,
        Position::NotSaid,
        "refs: {:?}",
        app.refs
    );
}

#[test]
fn a_remote_ref_at_a_checkout_is_not_a_second_ref_of_the_repositorys() {
    // A second ref at a path is answered with nothing, so a remote ref carrying a
    // `%(worktreepath)` would not overwrite the marker but take it away.
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
        tree.repos[0].worktrees[0].position,
        Position::Said(Some(Track::Gone)),
        "a remote ref is not a second ref at this checkout"
    );
}

#[test]
fn a_repository_key_spelled_with_a_trailing_slash_still_names_one_repository() {
    // The repository half of the key. Spelled two ways it would be two repositories, and
    // the checkout would lose its marker with nothing said.
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
    assert_eq!(
        tree.repos[0].worktrees[0].position,
        Position::Said(Some(Track::Gone))
    );
}

#[test]
fn a_checkout_spelled_one_way_by_git_and_another_by_herdr_is_one_checkout() {
    // The path half of the key. `%(worktreepath)` comes from git and `path` from herdr,
    // so either can be the one carrying the slash.
    for (herdr_says, git_says) in [("/wt/shared/", "/wt/shared"), ("/wt/shared", "/wt/shared/")] {
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
            tree.repos[0].worktrees[0].position,
            Position::Said(Some(Track::Gone)),
            "herdr said {herdr_says}, git said {git_says}"
        );
    }
}

#[test]
fn a_panes_repository_key_spelled_with_a_trailing_slash_still_reaches_its_refs() {
    // The repository half of the key at the row `build` makes for a pane.
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
    assert_eq!(synthesized.position, Position::Said(Some(Track::Gone)));
}

#[test]
fn a_repository_key_herdr_and_the_placement_spell_differently_is_one_repository() {
    // `RepoInput::repo_key` and `PanePlacement::repo_key` are two fields, and every site
    // that reads either normalizes it.
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
    assert_eq!(synthesized.position, Position::Said(Some(Track::Gone)));
}

#[test]
fn a_panes_own_row_is_read_from_the_checkout_the_pane_is_in() {
    // The checkout half of the key at the row `build` makes for a pane: a stale `[gone]`
    // naming some other path of the same repository must not land here. That is issue
    // #31's shape inside one repository.
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
        synthesized.position,
        Position::NotSaid,
        "git names no ref at the checkout this pane is in"
    );
}

#[test]
fn two_refs_that_agree_about_where_they_stand_are_contested_all_the_same() {
    // Two refs at one path disagree about which branch is there whatever their tracks
    // say, and both `[gone]` is the ordinary way it happens. A rule that refused only on
    // differing tracks would hand the row a `gone` built out of a contradiction.
    let shared = "/wt/shared";
    let mut app = repo(
        "me/app",
        "/src/app",
        vec![worktree("feat/login", shared, true)],
    );
    app.refs = Ok(vec![
        local_ref("feat/login", Some(shared), Some(Track::Gone)),
        local_ref("chore/deps", Some(shared), Some(Track::Gone)),
    ]);

    let tree = build(&snapshot(json!([])), &[app], &HashMap::new());
    assert_eq!(tree.repos[0].worktrees[0].position, Position::Contested);
}

#[test]
fn what_a_branchless_row_draws_turns_on_whether_herdr_listed_it() {
    // The two rows `build` makes, over one repository's identical git facts: a stale
    // registration goes on naming a path for `chore/deps`, whose upstream was deleted,
    // and nothing is checked out there. Neither row names a branch, and the two say so
    // differently: herdr spoke about the one it listed, and nobody spoke about the one
    // `build` made for a pane. Only the second draws `gone` about a branch it never
    // names — issue #49, pinned here so the difference stays deliberate.
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
        (&listed.branch, &unlisted.branch),
        (&Branch::NothingOut, &Branch::NotSaid),
        "the two rows say who was silent"
    );
    assert_eq!(
        (listed.branch.name(), unlisted.branch.name()),
        (None, None),
        "and neither names a branch"
    );
    assert_eq!(
        (listed.position, unlisted.position),
        (Position::NotSaid, Position::Said(Some(Track::Gone))),
        "and only the one herdr spoke about is refused the marker"
    );
}

#[test]
fn a_checkout_herdr_flags_detached_draws_no_track_whatever_it_names() {
    // `branch` is not the only way herdr can say nothing is out: `is_detached` is beside
    // it, and a herdr that fills `branch` for a detached checkout — with `HEAD`, a
    // commit, anything — must not get past the guard.
    let shared = "/wt/shared";
    let mut app = repo("me/app", "/src/app", vec![]);
    app.worktrees = vec![serde_json::from_value(json!({
        "branch": "HEAD",
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
    assert_eq!(
        row.branch,
        Branch::NothingOut,
        "detached wins over whatever herdr named"
    );
    assert_eq!(row.position, Position::NotSaid);
}
