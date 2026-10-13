use super::*;
use crate::app::fakes::{fake_git, FakeGit};
use crate::domain::model::{Branch, PaneNode, Unlisted, WorktreeNode};
use crate::domain::settings::{Panes, Settings};
use crate::port::{AgentStatus, Track};
use serde_json::json;
use std::num::NonZeroU32;
use std::path::PathBuf;

fn snapshot() -> Snapshot {
    serde_json::from_value(json!({
        "version": "0.7.4",
        "protocol": 16,
        "workspaces": [],
        "tabs": [],
        "panes": [{
            "pane_id": "w1:p1",
            "tab_id": "w1:t1",
            "workspace_id": "w1",
            "terminal_id": "term_w1:p1",
            "focused": false,
            "agent": "claude",
            "agent_status": "idle",
        }],
    }))
    .expect("snapshot fixture should deserialize")
}

fn plugin_config(path: Option<&str>, show_no_panes: bool, complaint: Option<&str>) -> Loaded {
    Loaded {
        settings: Settings {
            panes: Panes {
                show_worktrees_without_panes: show_no_panes,
            },
        },
        complaint: complaint.map(str::to_string),
        path: path.map(PathBuf::from),
    }
}

fn worktree(branch: Option<&str>, path: &str, track: Option<Track>) -> WorktreeNode {
    WorktreeNode {
        // A marker where the picker's walk found one, and otherwise nothing it found: the
        // tests about a walk that found a ref with nothing to report say
        // `Position::Said(None)` themselves, that being the case this page turns into
        // `level` or `none`. A row naming no branch carries no position at all.
        branch: branch.map_or(Branch::NothingOut, |name| Branch::Out {
            name: name.to_string(),
            position: track.map_or(Position::NotSaid, |track| Position::Said(Some(track))),
        }),
        checkout_path: path.to_string(),
        is_primary: branch == Some("main"),
        open_workspace_id: None,
        panes: Vec::new(),
    }
}

/// A checkout the picker's walk found one ref for and git had nothing to report about,
/// which this page turns into `track level` beside an upstream and `track none` without
/// one. `worktree`'s own `None` is the other silence: the walk found no ref here at all.
fn nothing_to_report(branch: &str, path: &str) -> WorktreeNode {
    let mut row = worktree(Some(branch), path, None);
    row.branch.set_position(Position::Said(None));
    row
}

/// A checkout the picker's walk found more than one ref for, and would not choose between.
fn contested(branch: &str, path: &str) -> WorktreeNode {
    let mut row = worktree(Some(branch), path, None);
    row.branch.set_position(Position::Contested);
    row
}

/// The row `domain::tree::build` makes for a pane in a checkout herdr never listed: the
/// one branchless row a branch may be out at.
fn unlisted(path: &str) -> WorktreeNode {
    WorktreeNode {
        branch: Branch::NotSaid,
        ..worktree(None, path, None)
    }
}

fn local(name: &str, worktree_path: &str, upstream: Option<&str>) -> GitRef {
    GitRef {
        name: name.to_string(),
        kind: RefKind::Local,
        committed_at: None,
        subject: None,
        upstream: upstream.map(str::to_string),
        track: None,
        worktree_path: Some(worktree_path.to_string()),
    }
}

#[test]
fn every_answer_a_row_leaves_out_is_on_the_page() {
    // Level and no upstream are the same empty marker on a row; unreadable refs draw
    // nothing where `gone` would go, and an unreadable working tree draws only `?`.
    let mut main = nothing_to_report("main", "/src/app");
    main.open_workspace_id = Some("w1".into());
    main.panes = vec![PaneNode {
        pane_id: "w1:p1".into(),
        workspace_id: "w1".into(),
        tab_id: "w1:t1".into(),
        display_name: Some("claude".into()),
        agent_status: AgentStatus::Idle,
        focused: false,
    }];
    let tree = Tree {
        repos: vec![
            RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                refs: Refs::Read,
                worktrees: vec![
                    main,
                    nothing_to_report("feat/login", "/wt/feat-login"),
                    worktree(Some("fix/crash"), "/wt/fix-crash", Some(Track::Gone)),
                    worktree(
                        Some("chore/deps"),
                        "/wt/chore-deps",
                        Some(Track::Ahead(NonZeroU32::new(2).unwrap())),
                    ),
                    worktree(None, "/wt/detached", None),
                ],
            },
            RepoNode {
                repo_key: "/src/site/.git".into(),
                repo_root: "/src/site".into(),
                display_name: "me/site".into(),
                refs: Refs::Unreadable("fatal: bad ref (`git for-each-ref …`)".into()),
                worktrees: vec![worktree(Some("develop"), "/wt/develop", None)],
            },
        ],
        ungrouped: Vec::new(),
        ..Default::default()
    };
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![
            local("main", "/src/app", Some("origin/main")),
            // Level with nothing: never pushed, no upstream configured.
            local("feat/login", "/wt/feat-login", None),
            local("fix/crash", "/wt/fix-crash", Some("origin/fix/crash")),
            // Pushed without `-u`: the track came off `push:track`.
            local("chore/deps", "/wt/chore-deps", None),
        ]),
    )]);
    let working_trees = WorkingTrees::from([
        ("/src/app".to_string(), Ok(false)),
        ("/wt/feat-login".to_string(), Ok(true)),
        (
            "/wt/fix-crash".to_string(),
            Err("fatal: not a git repository (`git status`)".to_string()),
        ),
        ("/wt/chore-deps".to_string(), Ok(false)),
        ("/wt/develop".to_string(), Ok(false)),
    ]);

    let page = report(
        &snapshot(),
        &Chrome::default(),
        &plugin_config(Some("/plugin/config.toml"), false, None),
        &tree,
        &refs,
        &working_trees,
    );
    let expected = "\
herdr 0.7.4 (protocol 16)
chrome: accent Named(Cyan), indicators Dots
plugin config: /plugin/config.toml
[panes].show_worktrees_without_panes: false
1 panes in 2 repos

me/app  [/src/app]
  * main open in w1  /src/app
      upstream origin/main  track level  working tree clean
      w1:p1  Idle  claude
  - feat/login  /wt/feat-login
      upstream none  track none  working tree dirty
  - fix/crash  /wt/fix-crash
      upstream origin/fix/crash  track gone  working tree unreadable: fatal: not a git repository (`git status`)
  - chore/deps  /wt/chore-deps
      upstream none  track \u{2191}2  working tree clean
  - detached  /wt/detached
      no branch reported  working tree not asked

me/site  [/src/site]
  refs unreadable: fatal: bad ref (`git for-each-ref …`)
  - develop  /wt/develop
      upstream not read  track not read  working tree clean
";
    assert_eq!(page, expected, "got:\n{page}");
}

#[test]
fn a_missing_plugin_file_is_reported_as_defaults() {
    let tree = Tree {
        repos: Vec::new(),
        ungrouped: Vec::new(),
        ..Default::default()
    };
    let page = report(
        &snapshot(),
        &Chrome::default(),
        &plugin_config(None, true, None),
        &tree,
        &RefsByRepo::new(),
        &WorkingTrees::new(),
    );
    assert!(
        page.contains("plugin config: missing, defaults\n"),
        "{page}"
    );
    assert!(
        page.contains("[panes].show_worktrees_without_panes: true\n"),
        "{page}"
    );
}

#[test]
fn a_rejected_plugin_file_names_the_problem() {
    let tree = Tree {
        repos: Vec::new(),
        ungrouped: Vec::new(),
        ..Default::default()
    };
    let page = report(
        &snapshot(),
        &Chrome::default(),
        &plugin_config(
            Some("/plugin/config.toml"),
            true,
            Some("plugin config.toml: unknown field `pane`"),
        ),
        &tree,
        &RefsByRepo::new(),
        &WorkingTrees::new(),
    );
    assert!(
        page.contains("plugin config problem: plugin config.toml: unknown field `pane`\n"),
        "{page}"
    );
}

/// A git that answers both of this page's questions, and records what it was asked.
struct Asked {
    walks: std::sync::Mutex<Vec<String>>,
    walk: Walk,
}

/// How the second walk goes: whole, short of a ref git could not read, or refused.
#[derive(Clone, Copy)]
enum Walk {
    Whole,
    Dropped,
    Refused,
}

impl FakeGit for Asked {
    fn local_refs(&self, repo_root: &str) -> Result<crate::port::RefWalk, anyhow::Error> {
        self.walks.lock().unwrap().push(repo_root.to_string());
        let refs = vec![local("main", "/src/app", Some("origin/main"))];
        match self.walk {
            Walk::Refused => {
                anyhow::bail!("fatal: bad ref for\n  refs/heads/x (`git for-each-ref …`)")
            }
            Walk::Dropped => Ok(crate::port::RefWalk {
                refs,
                dropped: Some("warning: ignoring broken ref\n  refs/heads/x".to_string()),
            }),
            Walk::Whole => Ok(crate::port::RefWalk::of(refs)),
        }
    }

    fn is_dirty(&self, checkout_path: &str) -> Result<bool, anyhow::Error> {
        match checkout_path {
            "/src/app" => Ok(true),
            _ => anyhow::bail!("fatal: not a git repository\n  (`git status`)"),
        }
    }
}
fake_git!(Asked);

fn asked(walk: Walk) -> Asked {
    Asked {
        walks: std::sync::Mutex::new(Vec::new()),
        walk,
    }
}

#[test]
fn what_is_asked_again_is_asked_about_the_repository_the_answer_is_filed_under() {
    // A key that is not `repo_root` is no error and nothing on screen: every checkout
    // would simply read `upstream not read`, which is what this page says when the
    // second walk failed.
    let tree = one_repo(Refs::Read, vec![nothing_to_report("main", "/src/app")]);
    let git = asked(Walk::Whole);
    let refs = read_refs(&git, &tree);
    assert_eq!(git.walks.lock().unwrap().as_slice(), ["/src/app"]);
    let page = report(
        &snapshot(),
        &Chrome::default(),
        &plugin_config(None, true, None),
        &tree,
        &refs,
        &read_working_trees(&git, &tree),
    );
    assert!(
        page.contains("      upstream origin/main  track level  working tree dirty"),
        "got:\n{page}"
    );
}

#[test]
fn a_repository_the_tree_could_not_read_is_not_asked_a_second_time() {
    // Its words are already on the page from the first walk, and a second
    // `for-each-ref` per repository is what this mode pays for the upstream names.
    let tree = one_repo(
        Refs::Unreadable("fatal: bad ref (`git for-each-ref …`)".to_string()),
        vec![worktree(Some("main"), "/src/app", None)],
    );
    let git = asked(Walk::Whole);
    assert!(read_refs(&git, &tree).is_empty());
    assert!(git.walks.lock().unwrap().is_empty(), "git was not asked");

    // Its working trees are still walked: whether a checkout is holding work is a
    // different question from what its branch tracks.
    assert_eq!(
        read_working_trees(&git, &tree).get("/src/app"),
        Some(&Ok(true))
    );
}

#[test]
fn a_second_walk_that_dropped_a_ref_or_would_not_answer_is_quoted_on_one_line() {
    // A walk missing a ref is not this repository's refs, here as in the picker; and
    // git's words are folded, because a sentence that keeps its newlines indents its
    // tail under the repository like a checkout of its own.
    let tree = one_repo(Refs::Read, vec![nothing_to_report("main", "/src/app")]);
    let git = asked(Walk::Dropped);
    assert_eq!(
        read_refs(&git, &tree).get("/src/app"),
        Some(&Err("warning: ignoring broken ref refs/heads/x".to_string()))
    );
    assert_eq!(
        read_working_trees(&git, &tree).get("/wt/nowhere"),
        None,
        "only the checkouts the tree has"
    );

    // And the working tree git would not walk, whose words reach the same one line.
    let tree = one_repo(Refs::Read, vec![worktree(Some("x"), "/wt/x", None)]);
    assert_eq!(
        read_working_trees(&git, &tree).get("/wt/x"),
        Some(&Err(
            "fatal: not a git repository (`git status`)".to_string()
        ))
    );
}

#[test]
fn a_second_walk_git_refused_is_what_the_second_read_line_is_drawn_from() {
    // The tree read these refs — the picker is drawing `gone` from that read — so
    // saying `refs unreadable` would make the picker look wrong where it is right.
    let tree = one_repo(
        Refs::Read,
        vec![worktree(
            Some("fix/crash"),
            "/wt/fix-crash",
            Some(Track::Gone),
        )],
    );
    let git = asked(Walk::Refused);
    let refs = read_refs(&git, &tree);
    assert_eq!(
        refs.get("/src/app"),
        Some(&Err(
            "fatal: bad ref for refs/heads/x (`git for-each-ref …`)".to_string()
        )),
        "git's words, on one line"
    );

    let page = report(
        &snapshot(),
        &Chrome::default(),
        &plugin_config(None, true, None),
        &tree,
        &refs,
        &read_working_trees(&git, &tree),
    );
    assert!(
        page.contains(
            "  refs unreadable on the second read: fatal: bad ref for refs/heads/x \
                 (`git for-each-ref …`)\n"
        ),
        "got:\n{page}"
    );
    assert!(
        page.contains("      upstream not read  track gone  working tree"),
        "and the track is still the tree's: {page}"
    );
}

fn one_repo(refs: Refs, worktrees: Vec<WorktreeNode>) -> Tree {
    Tree {
        repos: vec![RepoNode {
            repo_key: "/src/app/.git".into(),
            repo_root: "/src/app".into(),
            display_name: "me/app".into(),
            refs,
            worktrees,
        }],
        ungrouped: Vec::new(),
        ..Default::default()
    }
}

fn page(tree: &Tree, refs: &RefsByRepo) -> String {
    report(
        &snapshot(),
        &Chrome::default(),
        &plugin_config(None, true, None),
        tree,
        refs,
        &WorkingTrees::new(),
    )
}

#[test]
fn a_field_that_could_not_be_read_is_not_printed_as_a_branch_that_is_level() {
    // `level` is a positive claim — the branch is even with the ref git named — and this
    // is the page a bug report is made of. The picker draws the same blank for both, so
    // there is nowhere else for a reader to find out which of the two they have.
    let tree = one_repo(
        Refs::Read,
        vec![worktree(Some("main"), "/src/app", Some(Track::Unreadable))],
    );
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![local("main", "/src/app", Some("origin/main"))]),
    )]);
    assert!(
        page(&tree, &refs).contains("      upstream origin/main  track unreadable  working tree"),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_ref_at_the_same_checkout_spelled_with_a_trailing_slash_still_matches() {
    // git and herdr spell a path differently at the edge, and the tree matched them
    // through `normalize_path`; without it the page prints `none` for a branch that has
    // an upstream.
    let tree = one_repo(Refs::Read, vec![nothing_to_report("main", "/src/app")]);
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![local("main", "/src/app/", Some("origin/main"))]),
    )]);
    assert!(
        page(&tree, &refs).contains("      upstream origin/main  track level  working tree"),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_second_read_that_failed_says_so_and_keeps_the_track_the_picker_draws() {
    // Only this page's own second read failed. Saying `track not read` would make the
    // picker look wrong when it is right, which is the one thing a troubleshooting page
    // must not do.
    let tree = one_repo(
        Refs::Read,
        vec![
            worktree(Some("fix/crash"), "/wt/fix-crash", Some(Track::Gone)),
            worktree(Some("main"), "/src/app", None),
            contested("feat/login", "/wt/feat-login"),
        ],
    );
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Err("fatal: bad ref for refs/heads/x (`git for-each-ref …`)".to_string()),
    )]);
    let page = page(&tree, &refs);
    assert!(
        page.contains(
            "me/app  [/src/app]\n  refs unreadable on the second read: fatal: bad ref for \
                 refs/heads/x (`git for-each-ref …`)\n"
        ),
        "got:\n{page}"
    );
    assert!(
        page.contains("      upstream not read  track gone  working tree"),
        "the track is the tree's: {page}"
    );
    assert!(
        page.contains("      upstream not read  track not known  working tree"),
        "and an empty marker stays unnarrowed without the upstream: {page}"
    );
    assert!(
        page.contains("      upstream not read  track contested  working tree"),
        "and a refusal stays the picker's refusal: {page}"
    );

    // Not asked at all is the same page: nothing to quote, the track still the tree's.
    let page = self::page(&tree, &RefsByRepo::new());
    assert!(!page.contains("second read"), "nothing to quote: {page}");
    assert!(
        page.contains("      upstream not read  track gone  working tree"),
        "{page}"
    );
}

#[test]
fn a_ref_git_lists_no_checkout_for_does_not_answer_for_one() {
    // Most refs in a repository are branches nobody has checked out, and a remote ref
    // never has a checkout at all: git prints an empty `%(worktreepath)` for both. A
    // lookup that accepted them would hand the first ref in the list to every checkout.
    let tree = one_repo(Refs::Read, vec![nothing_to_report("main", "/src/app")]);
    let mut unchecked = local("feat/other", "/wt/feat-other", Some("origin/feat/other"));
    unchecked.worktree_path = None;
    let mut remote = local("main", "/src/app", Some("origin/nowhere"));
    remote.kind = RefKind::Remote;
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        // Ahead of `main` in the list, so a lookup taking the first match takes one of
        // these two.
        Ok(vec![
            unchecked,
            remote,
            local("main", "/src/app", Some("origin/main")),
        ]),
    )]);
    assert!(
        page(&tree, &refs).contains("      upstream origin/main  track level  working tree"),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_branch_git_lists_at_another_checkout_is_not_this_checkout_s_ref() {
    // Same name, different path: herdr says `/wt/feat-login` is on `feat/login` and git
    // says the branch is checked out somewhere else. Matched by name, this page would
    // print that other checkout's upstream here and say nothing was wrong.
    let tree = one_repo(
        Refs::Read,
        vec![worktree(Some("feat/login"), "/wt/feat-login", None)],
    );
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![local(
            "feat/login",
            "/elsewhere/feat-login",
            Some("origin/feat/login"),
        )]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      no ref at this checkout for feat/login  track not known  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_checkout_git_lists_no_ref_at_is_not_called_a_branch_nobody_pushed() {
    // The words `upstream none` are what a never-pushed branch reads as, and this is
    // not that: it is git and herdr disagreeing, which is the thing to go and look at.
    let tree = one_repo(
        Refs::Read,
        vec![worktree(Some("feat/login"), "/wt/feat-login", None)],
    );
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![local("main", "/src/app", Some("origin/main"))]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      no ref at this checkout for feat/login  track not known  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_repository_herdr_would_not_list_has_a_heading_of_its_own() {
    // It has no section above — that is what "not listed" means — so the page would
    // otherwise say nothing about a repository the reader can see panes for.
    let mut tree = one_repo(Refs::Read, Vec::new());
    tree.trouble.unlisted.push(Unlisted {
        repo_key: "/src/old/.git".into(),
        words: "herdr rejected worktree.list: internal error".into(),
        panes: Default::default(),
    });
    let heading = page(&tree, &RefsByRepo::new());
    assert!(
        heading.contains("\nnot listed:\n  old  [/src/old/.git]\n"),
        "got:\n{heading}"
    );
    assert!(
        heading.contains("      herdr rejected worktree.list: internal error\n"),
        "got:\n{heading}"
    );

    // And its panes, which went under the heading below: nothing else on the page ties them
    // to the section above, and that heading is also where a pane herdr cannot see into goes.
    tree.trouble.unlisted[0]
        .panes
        .insert("w7:p1".into(), "/src/old".into());
    tree.ungrouped = vec![PaneNode {
        pane_id: "w7:p1".into(),
        workspace_id: "w7".into(),
        tab_id: "w7:t1".into(),
        display_name: None,
        agent_status: AgentStatus::Unknown,
        focused: false,
    }];
    let with_panes = page(&tree, &RefsByRepo::new());
    assert!(
        with_panes.ends_with(
            "\nnot in any repository:\n      w7:p1\n          in old, which is not listed\n"
        ),
        "got:\n{with_panes}"
    );
}

#[test]
fn panes_in_no_repository_are_listed_last_under_their_own_heading() {
    let mut tree = one_repo(Refs::Read, Vec::new());
    tree.ungrouped = vec![PaneNode {
        pane_id: "w9:p9".into(),
        workspace_id: "w9".into(),
        tab_id: "w9:t1".into(),
        display_name: None,
        agent_status: AgentStatus::Unknown,
        focused: false,
    }];
    let bare = page(&tree, &RefsByRepo::new());
    assert!(
        bare.ends_with("\nnot in any repository:\n      w9:p9\n"),
        "got:\n{bare}"
    );

    // With a reason, where there is one. herdr not seeing into the pane and git not
    // answering about it end up under the same heading, and this page exists to tell
    // one from the other.
    tree.trouble.unplaced.insert(
        "w9:p9".to_string(),
        "git could not be run: no such file or directory (`git rev-parse`)".to_string(),
    );
    let with_reason = page(&tree, &RefsByRepo::new());
    assert!(
        with_reason.ends_with(
            "\nnot in any repository:\n      w9:p9\n          git could not be run: \
             no such file or directory (`git rev-parse`)\n"
        ),
        "got:\n{with_reason}"
    );
}

#[test]
fn a_ref_with_no_upstream_among_the_others_says_so_twice_over() {
    // Both halves of the last entry read `none`, meaning two different things: git
    // named no upstream, and git had nothing to report about where it stands against
    // the one it would push to. The compact line has no labels to keep them apart.
    let tree = one_repo(Refs::Read, vec![contested("feat/login", "/wt/shared")]);
    let mut deps = local("chore/deps", "/wt/shared", Some("origin/chore/deps"));
    deps.track = Some(Track::Gone);
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![
            local("feat/login", "/wt/shared", Some("origin/feat/login")),
            deps,
            local("scratch", "/wt/shared", None),
        ]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      more than one ref at this checkout: \
                 feat/login \u{2192} origin/feat/login level, \
                 chore/deps \u{2192} origin/chore/deps gone, \
                 scratch \u{2192} none none  \
                 track contested  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn two_refs_where_the_picker_saw_one_are_two_reads_disagreeing() {
    // The picker's walk and this page's second walk are two walks, so this one can see
    // a second ref where that one saw only the marked ref. The count is the row's (#47):
    // a headline of `more than one ref at this checkout` would be this page's walk
    // standing in for the picker's, and the marker the picker is drawing stays.
    let tree = one_repo(
        Refs::Read,
        vec![worktree(
            Some("feat/login"),
            "/wt/shared",
            Some(Track::Gone),
        )],
    );
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![
            local("feat/login", "/wt/shared", Some("origin/feat/login")),
            local("chore/deps", "/wt/shared", Some("origin/chore/deps")),
        ]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      the picker's read named one ref at this checkout and this one names \
                 feat/login \u{2192} origin/feat/login level, \
                 chore/deps \u{2192} origin/chore/deps level  \
                 track gone  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_marker_the_picker_drew_survives_a_walk_that_names_no_ref_here() {
    // Where the second walk names no ref at the path, the marker the first drew is
    // still the marker on the row, and the line says the reads disagree rather than that
    // there is no ref here.
    let tree = one_repo(
        Refs::Read,
        vec![worktree(
            Some("feat/login"),
            "/wt/shared",
            Some(Track::Gone),
        )],
    );
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![local("main", "/src/app", Some("origin/main"))]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      the picker's read named one ref at this checkout and this one names none  \
                 track gone  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_ref_with_no_upstream_but_a_marker_keeps_its_marker_in_the_list() {
    // A branch with no upstream is measured against where it would push —
    // `a_branch_with_no_upstream_is_still_measured_against_where_it_would_push` in
    // `tests/git_adapter.rs` — so its entry reads as no upstream and ahead of the push
    // destination. The compact form must at least not drop the measurement.
    let tree = one_repo(Refs::Read, vec![contested("feat/login", "/wt/shared")]);
    let mut scratch = local("scratch", "/wt/shared", None);
    scratch.track = Some(Track::Ahead(NonZeroU32::new(2).unwrap()));
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![
            local("feat/login", "/wt/shared", Some("origin/feat/login")),
            scratch,
        ]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      more than one ref at this checkout: \
                 feat/login \u{2192} origin/feat/login level, \
                 scratch \u{2192} none \u{2191}2  \
                 track contested  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_checkout_with_nothing_out_names_the_refs_git_still_has_at_its_path() {
    // Named the same way the multi-ref line names them, and without saying what it
    // means, because on this row the page cannot know: see
    // `a_checkout_herdr_never_listed_says_so_rather_than_that_no_branch_is_out`.
    let tree = one_repo(Refs::Read, vec![worktree(None, "/wt/shared", None)]);
    let mut deps = local("chore/deps", "/wt/shared", Some("origin/chore/deps"));
    deps.track = Some(Track::Gone);
    let refs = RefsByRepo::from([("/src/app".to_string(), Ok(vec![deps]))]);
    assert!(
        page(&tree, &refs).contains(
            "      no branch reported  git names at this path: \
                 chore/deps \u{2192} origin/chore/deps gone  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_checkout_with_nothing_out_under_refs_that_were_not_read_says_so() {
    // No `upstream not read`, which is what a row with a branch reads: there is no
    // branch here for an upstream to be about. What is missing is the ref list.
    let tree = one_repo(
        Refs::Unreadable("fatal: bad ref (`git for-each-ref …`)".into()),
        vec![worktree(None, "/wt/shared", None)],
    );
    assert!(
        page(&tree, &RefsByRepo::new())
            .contains("      no branch reported  refs not read  working tree"),
        "got:\n{}",
        page(&tree, &RefsByRepo::new())
    );
}

#[test]
fn a_checkout_with_nothing_out_says_which_read_of_the_refs_failed() {
    // Saying `refs not read` here — which is what a repository nobody could read at
    // all reads as, in the test above — puts the row under a header that contradicts
    // it.
    let tree = one_repo(Refs::Read, vec![worktree(None, "/wt/shared", None)]);
    let second_read_failed = RefsByRepo::from([(
        "/src/app".to_string(),
        Err("fatal: bad object HEAD (`git for-each-ref …`)".to_string()),
    )]);
    let page = page(&tree, &second_read_failed);
    assert!(
        page.contains("  refs unreadable on the second read: fatal: bad object HEAD"),
        "got:\n{page}"
    );
    assert!(
        page.contains("      no branch reported  refs not read on the second read  working tree"),
        "got:\n{page}"
    );
}

#[test]
fn a_checkout_herdr_never_listed_says_so_rather_than_that_no_branch_is_out() {
    // The row `build` makes for a pane in a checkout herdr never listed. `no branch
    // reported` would be this side's inference in herdr's voice: `git worktree add` outside
    // herdr leaves a branch out at a path herdr's list never mentions, and the ref named
    // below may well be it. The row carries no position (issue #49), so the ref is named
    // and not explained.
    let tree = one_repo(Refs::Read, vec![unlisted("/wt/feature")]);
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![local(
            "feat/login",
            "/wt/feature",
            Some("origin/feat/login"),
        )]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      herdr did not list this checkout  git names at this path: \
                 feat/login \u{2192} origin/feat/login level  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_checkout_herdr_never_listed_says_which_read_of_the_refs_failed() {
    // As a checkout with nothing out does in
    // `a_checkout_with_nothing_out_says_which_read_of_the_refs_failed`, with herdr's silence
    // in place of its answer.
    let tree = one_repo(Refs::Read, vec![unlisted("/wt/shared")]);
    let second_read_failed = RefsByRepo::from([(
        "/src/app".to_string(),
        Err("fatal: bad object HEAD (`git for-each-ref …`)".to_string()),
    )]);
    assert!(
        page(&tree, &second_read_failed).contains(
            "      herdr did not list this checkout  refs not read on the second read  working tree"
        ),
        "got:\n{}",
        page(&tree, &second_read_failed)
    );
}

#[test]
fn a_checkout_the_picker_would_not_choose_a_ref_for_says_so_rather_than_level() {
    // The two walks disagree the other way round from
    // `two_refs_where_the_picker_saw_one_are_two_reads_disagreeing`: this page's walk names
    // one ref here and the picker's named two. Read off its own walk alone, this page would
    // print `upstream origin/main  track level` — the claim `domain::tree::tracks` refused
    // to make. It reads the row's count instead (#47), and names the one ref it found.
    let tree = one_repo(Refs::Read, vec![contested("feat/login", "/wt/shared")]);
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![local("feat/login", "/wt/shared", Some("origin/main"))]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      the picker's read named more than one ref at this checkout and this one \
             names feat/login \u{2192} origin/main level  track contested  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_walk_that_found_a_ref_and_no_position_is_not_a_walk_that_found_no_ref() {
    // Beside a list of refs there is no one upstream for `level` or `none` to be a claim
    // about, so the two silences read as themselves: git reported nothing about the one
    // ref the picker found, against the picker having found no ref here at all.
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![
            local("feat/login", "/wt/shared", Some("origin/feat/login")),
            local("chore/deps", "/wt/shared", Some("origin/chore/deps")),
        ]),
    )]);
    let both = "at this checkout and this one names \
                feat/login \u{2192} origin/feat/login level, \
                chore/deps \u{2192} origin/chore/deps level  ";
    let reported_nothing = one_repo(
        Refs::Read,
        vec![nothing_to_report("feat/login", "/wt/shared")],
    );
    assert!(
        page(&reported_nothing, &refs).contains(&format!(
            "the picker's read named one ref {both}track nothing reported  working"
        )),
        "got:\n{}",
        page(&reported_nothing, &refs)
    );
    let no_ref = one_repo(
        Refs::Read,
        vec![worktree(Some("feat/login"), "/wt/shared", None)],
    );
    assert!(
        page(&no_ref, &refs).contains(&format!(
            "the picker's read named no ref {both}track not known  working"
        )),
        "got:\n{}",
        page(&no_ref, &refs)
    );
}

#[test]
fn a_row_the_picker_found_no_ref_for_is_not_a_row_it_would_not_choose_one_for() {
    // This page has a word for each, because the two send somebody to different places:
    // a row the picker would not choose a ref for wants its stale registration pointed back
    // at its directory, and one it found no ref for wants a look at whether the branch has a
    // commit yet and what git and herdr say is checked out where.
    // `a_checkout_the_picker_would_not_choose_a_ref_for_says_so_rather_than_level` is the
    // first.
    let tree = one_repo(
        Refs::Read,
        vec![worktree(Some("feat/login"), "/wt/shared", None)],
    );
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![local("feat/login", "/wt/shared", Some("origin/main"))]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      the picker's read named no ref at this checkout and this one names \
             feat/login \u{2192} origin/main level  track not known  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn two_refs_at_a_row_with_no_branch_are_both_named() {
    // The list on this row is the same list the multi-ref line prints, all of it.
    let tree = one_repo(Refs::Read, vec![worktree(None, "/wt/shared", None)]);
    let mut deps = local("chore/deps", "/wt/shared", Some("origin/chore/deps"));
    deps.track = Some(Track::Gone);
    let refs = RefsByRepo::from([(
        "/src/app".to_string(),
        Ok(vec![
            local("feat/login", "/wt/shared", Some("origin/feat/login")),
            deps,
        ]),
    )]);
    assert!(
        page(&tree, &refs).contains(
            "      no branch reported  git names at this path: \
                 feat/login \u{2192} origin/feat/login level, \
                 chore/deps \u{2192} origin/chore/deps gone  working tree"
        ),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_row_with_no_branch_and_no_ref_reads_no_branch_reported_alone() {
    // Nothing after it: no ref for `git names at this path:` to list, and no position on a
    // row that names no branch.
    let tree = one_repo(Refs::Read, vec![worktree(None, "/wt/shared", None)]);
    let refs = RefsByRepo::from([("/src/app".to_string(), Ok(Vec::new()))]);
    assert!(
        page(&tree, &refs).contains("      no branch reported  working tree"),
        "got:\n{}",
        page(&tree, &refs)
    );
}

#[test]
fn a_checkout_a_ref_is_missing_from_carries_what_git_said_of_it() {
    // herdr lists the checkout under a directory in `refs/heads` git may not open as not
    // detached with no branch, and git's refusal is the only reason it gives. The page
    // prints it under the row's branch line, ahead of its panes, and the same for a named
    // branch git refused about. A checkout git found a commit at has no words of git's to
    // print, and the page says what git found instead.
    let mut locked = WorktreeNode {
        branch: Branch::Unnamed,
        ..unlisted("/wt/feat2-x")
    };
    locked.panes = vec![PaneNode {
        pane_id: "w2:p1".into(),
        workspace_id: "w2".into(),
        tab_id: "w2:t1".into(),
        display_name: Some("claude".into()),
        agent_status: AgentStatus::Idle,
        focused: false,
    }];
    let no_ref = |name: &str| WorktreeNode {
        branch: Branch::Out {
            name: name.into(),
            position: Position::NotSaid,
        },
        ..unlisted(&format!("/wt/{name}"))
    };
    let mut tree = one_repo(Refs::Read, vec![locked, no_ref("broken"), no_ref("moved")]);
    let words = "fatal: your current branch appears to be broken (`git log -1 --format=%H`)";
    let key = |worktree: usize| {
        (
            RepoKey::of(&tree.repos[0]),
            CheckoutPath::of(&tree.repos[0].worktrees[worktree]),
        )
    };
    let (locked, broken, moved) = (key(0), key(1), key(2));
    tree.trouble.ref_missing.insert(locked, Some(words.into()));
    tree.trouble.ref_missing.insert(broken, Some(words.into()));
    tree.trouble.ref_missing.insert(moved, None);
    let refs = RefsByRepo::from([("/src/app".to_string(), Ok(Vec::new()))]);
    let page = page(&tree, &refs);
    assert!(
        page.contains(&format!(
            "      no branch reported, not detached  working tree not asked\n      {words}\n      \
             w2:p1  Idle  claude\n"
        )),
        "got:\n{page}"
    );
    assert!(
        page.contains(&format!(
            "      no ref at this checkout for broken  track not known  working tree not \
             asked\n      {words}\n"
        )),
        "got:\n{page}"
    );
    assert!(
        page.ends_with(
            "      no ref at this checkout for moved  track not known  working tree \
             not asked\n      git found a commit here\n"
        ),
        "got:\n{page}"
    );
}
