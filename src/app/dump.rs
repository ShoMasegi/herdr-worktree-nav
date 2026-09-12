//! What the plugin currently sees, as text.
//!
//! `herdr-worktree-nav dump` is for the moment the picker shows something surprising: it
//! separates "herdr or git told us something odd" from "the UI drew it wrong". So it prints
//! what the rows are drawn from, and the answers the rows deliberately leave out — that git
//! would not read a repository's refs, that a branch has no upstream to be level with, and
//! why git would not read a working tree. Outside a sweep a row draws nothing for the first
//! two and only `?` for the third, because no marker beats a wrong one
//! (`domain::model::Refs`, `domain::model::WorkingTree`); in a sweep a row it would have
//! judged says `refs unreadable`, which is that something is missing rather than what. This
//! is where the difference is legible.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::app::one_line;
use crate::domain::chrome::Chrome;
use crate::domain::model::{normalize_path, Refs, RepoNode, Tree, WorktreeNode};
use crate::domain::rows;
use crate::port::{GitPort, GitRef, RefKind, Snapshot, Track};

/// What the second ref walk — the one `main` makes for this page, for the upstream names —
/// said for each repository whose first walk the tree already carries as read: the refs,
/// or git's words when it would not say them again. The page says so under the repository,
/// and its checkouts keep the track the tree read the first time. A repository whose first
/// walk failed is not asked again and is absent here; the tree says so on its own line.
pub type RefsByRepo = BTreeMap<String, Result<Vec<GitRef>, String>>;

/// What git said about each working tree, by checkout path, or its words when it would not
/// say. The picker keeps only that it would not — `WorkingTree::Unreadable` — and the words
/// are the half a person troubleshooting needs.
pub type WorkingTrees = BTreeMap<String, Result<bool, String>>;

/// Ask git again for what the rows are drawn from, and for what they leave out.
///
/// A second `for-each-ref` per repository, for the upstream names: `collect::collect_tree`
/// reads the refs and keeps what the rows draw, and the rows do not draw those. This is a
/// mode for troubleshooting, so a process more per repository is the right side of that
/// trade. A repository whose first walk went wrong is not asked again — the tree already
/// carries git's words for it — and one that goes wrong the second time has its words kept,
/// so the page can say that this read failed where the tree's did not.
pub fn read_refs(git: &dyn GitPort, tree: &Tree) -> RefsByRepo {
    tree.repos
        .iter()
        .filter(|repo| repo.refs.is_read())
        .map(|repo| {
            let read = match git.local_refs(&repo.repo_root) {
                Err(error) => Err(one_line(&format!("{error:#}"))),
                // The same call the tree made, so the same rule: a walk missing a ref is
                // not a list of this repository's refs — `port::RefWalk`.
                Ok(walk) => match walk.dropped {
                    Some(words) => Err(one_line(&words)),
                    None => Ok(walk.refs),
                },
            };
            (repo.repo_root.clone(), read)
        })
        .collect()
}

/// Walk every checkout's working tree, one after another.
///
/// The picker asks this on threads behind the first frame and keeps what git answered minus
/// its words — clean, dirty, or would not say. Here it is asked in the open, one checkout at a time, and the words
/// are kept: they are the half of the answer somebody troubleshooting is here for.
pub fn read_working_trees(git: &dyn GitPort, tree: &Tree) -> WorkingTrees {
    tree.repos
        .iter()
        .flat_map(|repo| &repo.worktrees)
        .map(|worktree| {
            let answer = git
                .is_dirty(&worktree.checkout_path)
                .map_err(|error| one_line(&format!("{error:#}")));
            (worktree.checkout_path.clone(), answer)
        })
        .collect()
}

/// The report, one repository at a time, each checkout with a line of its own for what git
/// said about it.
pub fn report(
    snapshot: &Snapshot,
    chrome: &Chrome,
    tree: &Tree,
    refs: &RefsByRepo,
    working_trees: &WorkingTrees,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "herdr {} (protocol {})",
        snapshot.version, snapshot.protocol
    );
    let _ = writeln!(
        out,
        "chrome: accent {:?}, indicators {:?}",
        chrome.accent, chrome.indicators
    );
    let _ = writeln!(
        out,
        "{} panes in {} repos",
        snapshot.panes.len(),
        tree.repos.len()
    );
    for repo in &tree.repos {
        let _ = writeln!(out, "\n{}  [{}]", repo.display_name, repo.repo_root);
        match (&repo.refs, refs.get(&repo.repo_root)) {
            (Refs::Unreadable(words), _) => {
                let _ = writeln!(out, "  refs unreadable: {words}");
            }
            (Refs::Read, Some(Err(words))) => {
                let _ = writeln!(out, "  refs unreadable on the second read: {words}");
            }
            (Refs::Read, Some(Ok(_)) | None) => {}
        }
        for worktree in &repo.worktrees {
            let open = match &worktree.open_workspace_id {
                Some(id) => format!(" open in {id}"),
                None => String::new(),
            };
            let _ = writeln!(
                out,
                "  {} {}{}  {}",
                if worktree.is_primary { "*" } else { "-" },
                worktree.label(),
                open,
                worktree.checkout_path
            );
            let _ = writeln!(
                out,
                "      {}  {}",
                branch_words(repo, worktree, refs),
                working_tree_words(working_trees.get(&worktree.checkout_path))
            );
            for pane in &worktree.panes {
                let _ = writeln!(
                    out,
                    "      {}  {:?}  {}",
                    pane.pane_id,
                    pane.agent_status,
                    pane.display_name.as_deref().unwrap_or("")
                );
            }
        }
    }
    if !tree.ungrouped.is_empty() {
        let _ = writeln!(out, "\nnot in any repository:");
        for pane in &tree.ungrouped {
            let _ = writeln!(out, "      {}", pane.pane_id);
        }
    }
    out
}

/// `upstream origin/x  track gone` — the two facts a marker is drawn from, in words the
/// marker cannot hold: `level` for a branch even with its upstream, `none` for one with no
/// upstream to be even with, and `not read` when git would not read the refs at all.
///
/// A branch with no upstream is measured against where it would push — see
/// `adapter::git_cli` — so `upstream none` is followed by the marker when it is ahead of
/// that, and by `none` otherwise: level with where it would push, nothing to measure it
/// against, or a push destination git called `[gone]`, which under `push.default = current`
/// is what a branch nobody has pushed gets and which the adapter drops rather than believe.
///
/// Three ways this page knows less than the picker, and says which. When only its own
/// second ref walk failed, the upstream is `not read` and the track is the tree's — the
/// marker the picker is drawing — or `level or none`, which is as far as an empty marker
/// narrows down without an upstream to read it against. `domain::tree` leaves an empty
/// marker for reasons beyond those two — a walk that named no ref at the path, or two that
/// did — and this page cannot tell those apart: see below.
/// When git listed no ref at this checkout —
/// its list and herdr's disagree about what is checked out where — the page says that
/// rather than `upstream none`, which is what a branch nobody has pushed reads as. And when
/// git named more than one, the page names each of them with its upstream and where it
/// stands — two more facts about the contradiction, not a way to resolve it — and the track
/// after them is the marker the picker is drawing, or `not known` where it is drawing none.
/// Which of them is the odd one out this page cannot say, and `git worktree list` shows
/// every registration and marks none. `git worktree prune` leaves one of them — not one of
/// two, because git does not stop at two:
/// `prune_takes_one_of_two_registrations_at_one_path` and
/// `prune_leaves_one_registration_where_three_name_one_path` in `tests/git_adapter.rs`
/// build both shapes and hold that much. Which one is left is deliberately not held,
/// because it came out differently there and on CI on the same git. Both troubleshooting
/// pages say the same and no more.
///
/// Every line here is read from two walks rather than one — the track from the picker's,
/// the upstream and the ref list from this page's own second `for-each-ref` — and nothing
/// on the page marks where the two have disagreed. Three shapes that come of it:
///
/// - A checkout [`crate::domain::tree`] declined to measure, because the picker's walk saw
///   two refs at it, reads `track level` here when this walk sees one ref with an upstream:
///   a measurement standing in for the refusal.
/// - The other way about, the picker's marker is printed after a list every entry of which
///   contradicts it — `more than one ref at this checkout: … level, … level  track gone`.
/// - `upstream none  track gone` is a pair no single walk produces. git reports `[gone]` on
///   `%(upstream:track)`, which only a branch with an upstream has, and on `%(push:track)`,
///   which `adapter::git_cli` refuses to believe.
///
/// Only the first reads as an ordinary measurement; the other two put the contradiction on
/// the page with nothing to name it. Marking any of them needs a fact
/// [`WorktreeNode::track`] cannot carry, which is issue #47.
fn branch_words(repo: &RepoNode, worktree: &WorktreeNode, refs: &RefsByRepo) -> String {
    let Some(branch) = worktree.branch.as_deref() else {
        return detached_words(repo, worktree, refs);
    };
    // The marker the picker is drawing, or `unmarked` where it is drawing none: what an
    // empty marker leaves open depends on why this page could not narrow it down itself.
    let tree_track = |unmarked: &str| match worktree.track {
        Some(track) => rows::track_mark(Some(track)).trim_start().to_string(),
        None => unmarked.to_string(),
    };
    let read = match (&repo.refs, refs.get(&repo.repo_root)) {
        (Refs::Unreadable(_), _) => return "upstream not read  track not read".to_string(),
        (Refs::Read, Some(Err(_)) | None) => {
            return format!("upstream not read  track {}", tree_track("level or none"))
        }
        (Refs::Read, Some(Ok(read))) => read,
    };
    let named = refs_at(read, &worktree.checkout_path);
    let git_ref = match named.as_slice() {
        [only] => only,
        [] => {
            return format!(
                "no ref at this checkout for {branch}  track {}",
                tree_track("level or none")
            )
        }
        more => {
            // Each with what git said about it, because the names alone are bare words a
            // reader has nothing to do with. Not a tell: which entry is stale is a fact
            // about git's worktree registrations and where an upstream went is a fact
            // about the remote, and nothing ties the two together. What this compact form
            // loses with the labels is issue #48.
            return format!(
                "more than one ref at this checkout: {}  track {}",
                each_of(more),
                tree_track("not known")
            );
        }
    };
    let upstream = git_ref.upstream.as_deref();
    let track = standing(worktree.track, upstream);
    format!("upstream {}  track {track}", upstream.unwrap_or("none"))
}

/// The local refs git says have this checkout out.
///
/// By the checkout rather than by the branch name, which is how the tree matched them too —
/// its refusal to choose between two included. `find` would take whichever came first and
/// print an upstream the empty marker beside it does not stand for. Local only, for the
/// reason `domain::tree::tracks` is: git puts no checkout on a remote ref, and the adapter
/// reads `%(worktreepath)` for both kinds before it decides which kind it has.
fn refs_at<'a>(read: &'a [GitRef], checkout_path: &str) -> Vec<&'a GitRef> {
    read.iter()
        .filter(|git_ref| git_ref.kind == RefKind::Local)
        .filter(|git_ref| {
            git_ref
                .worktree_path
                .as_deref()
                .is_some_and(|path| normalize_path(path) == checkout_path)
        })
        .collect()
}

/// Each of them as `<branch> → <upstream> <where it stands>`, because the names alone are
/// bare words a reader has nothing to do with. Not a tell: which entry is stale is a fact
/// about git's worktree registrations and where an upstream went is a fact about the
/// remote, and nothing ties the two together. What this compact form loses with the labels
/// is issue #48.
fn each_of(named: &[&GitRef]) -> String {
    named
        .iter()
        .map(|git_ref| {
            let upstream = git_ref.upstream.as_deref();
            format!(
                "{} \u{2192} {} {}",
                git_ref.name,
                upstream.unwrap_or("none"),
                standing(git_ref.track, upstream)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the page says about a row with no branch on it.
///
/// Which is not one kind of row. [`WorktreeNode::branch`] is `None` both where herdr listed
/// the checkout and said nothing is out at it, and where herdr never named the checkout at
/// all — and a checkout herdr never named can have a branch out, `domain::tree::build`'s
/// own comment naming the case (`git worktree add` run outside herdr). The word `detached`
/// covers both, because nothing on a `WorktreeNode` tells them apart; that shortage is
/// issue #28, and issue #49 is the marker half of it.
///
/// So the refs named here are named and not explained. Where herdr said nothing is out,
/// git going on naming a ref at the path is a worktree registration that lost its
/// directory, which is what `git worktree prune` is for. Where herdr never said, the same
/// line is git reporting the branch that is out. This page cannot tell you which it is
/// looking at, and saying so is better than picking one — the prune reading, written here
/// and on both troubleshooting pages until it was caught, sends a reader to prune a live
/// worktree.
///
/// `detached` alone is still what a row with nothing to add reads as. No `upstream …` here
/// whatever git says: this row names no branch for one to be about, and the refs below
/// carry their own.
fn detached_words(repo: &RepoNode, worktree: &WorktreeNode, refs: &RefsByRepo) -> String {
    let mut out = "detached".to_string();
    match (&repo.refs, refs.get(&repo.repo_root)) {
        // Told apart the way `branch_words` tells them apart: under `Unreadable` the picker
        // has no markers either, and under a failed second read it has the ones this page
        // cannot name. Collapsing the two put `refs not read` on a row three lines under a
        // header saying only the second read failed.
        (Refs::Unreadable(_), _) => out.push_str("  refs not read"),
        (Refs::Read, Some(Err(_)) | None) => out.push_str("  refs not read on the second read"),
        (Refs::Read, Some(Ok(read))) => {
            let named = refs_at(read, &worktree.checkout_path);
            if !named.is_empty() {
                let _ = write!(out, "  git names at this path: {}", each_of(&named));
            }
        }
    }
    if let Some(track) = worktree.track {
        let _ = write!(
            out,
            "  track {}",
            rows::track_mark(Some(track)).trim_start()
        );
    }
    out
}

/// Where a branch stands, in the words the marker cannot hold: what `rows::track_mark`
/// draws for what git reported, else `level` for a branch even with the upstream git named
/// and `none` for one with no upstream to be even with, which [`branch_words`]' paragraph
/// on push destinations spells out. Whose report it is the caller decides: this page's own
/// walk in the `more` arm, the picker's at the tail — and at the tail the picker's `None`
/// has readings that paragraph does not cover.
fn standing(track: Option<Track>, upstream: Option<&str>) -> String {
    match track {
        Some(track) => rows::track_mark(Some(track)).trim_start().to_string(),
        None if upstream.is_some() => "level".to_string(),
        None => "none".to_string(),
    }
}

/// `working tree clean`, `dirty`, `unreadable: <git's words>`, or `not asked`.
fn working_tree_words(answer: Option<&Result<bool, String>>) -> String {
    match answer {
        Some(Ok(false)) => "working tree clean".to_string(),
        Some(Ok(true)) => "working tree dirty".to_string(),
        Some(Err(words)) => format!("working tree unreadable: {words}"),
        None => "working tree not asked".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{PaneNode, WorktreeNode};
    use crate::port::{AgentStatus, Track};
    use serde_json::json;
    use std::num::NonZeroU32;

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

    fn worktree(branch: Option<&str>, path: &str, track: Option<Track>) -> WorktreeNode {
        WorktreeNode {
            branch: branch.map(str::to_string),
            checkout_path: path.to_string(),
            is_primary: branch == Some("main"),
            open_workspace_id: None,
            track,
            panes: Vec::new(),
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
        // Each is its own words here, with git's own words where git gave any.
        let mut main = worktree(Some("main"), "/src/app", None);
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
                        worktree(Some("feat/login"), "/wt/feat-login", None),
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
            &tree,
            &refs,
            &working_trees,
        );
        let expected = "\
herdr 0.7.4 (protocol 16)
chrome: accent Named(Cyan), indicators Dots
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
      detached  working tree not asked

me/site  [/src/site]
  refs unreadable: fatal: bad ref (`git for-each-ref …`)
  - develop  /wt/develop
      upstream not read  track not read  working tree clean
";
        assert_eq!(page, expected, "got:\n{page}");
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

    impl GitPort for Asked {
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
        fn github_slug(
            &self,
            _repo_root: &str,
        ) -> Result<Option<crate::port::Slug>, anyhow::Error> {
            unreachable!("this page asks git two things")
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>, anyhow::Error> {
            unreachable!("this page asks git two things")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>, anyhow::Error> {
            unreachable!("this page asks git two things")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<(), anyhow::Error> {
            unreachable!("this page asks git two things")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<(), anyhow::Error> {
            unreachable!("this page asks git two things")
        }
        fn remove_worktree(
            &self,
            _repo_root: &str,
            _checkout_path: &str,
        ) -> Result<(), anyhow::Error> {
            unreachable!("this page asks git two things")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String, anyhow::Error> {
            unreachable!("this page asks git two things")
        }
    }

    fn asked(walk: Walk) -> Asked {
        Asked {
            walks: std::sync::Mutex::new(Vec::new()),
            walk,
        }
    }

    #[test]
    fn what_is_asked_again_is_asked_about_the_repository_the_answer_is_filed_under() {
        // The page reads an answer back by `repo_root`, and a key that is not that one is
        // no error and nothing on screen: every checkout would simply read `upstream not
        // read`, which is what this page says when the second walk failed. So both halves
        // are pinned here — that git is asked, and that the answer comes back.
        let tree = one_repo(Refs::Read, vec![worktree(Some("main"), "/src/app", None)]);
        let git = asked(Walk::Whole);
        let refs = read_refs(&git, &tree);
        assert_eq!(git.walks.lock().unwrap().as_slice(), ["/src/app"]);
        let page = report(
            &snapshot(),
            &Chrome::default(),
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
        // Its words are already on the page from the first walk, and a second `for-each-ref`
        // per repository is what this mode pays for the upstream names. There is nothing to
        // learn from asking a repository that has already refused.
        let tree = one_repo(
            Refs::Unreadable("fatal: bad ref (`git for-each-ref …`)".to_string()),
            vec![worktree(Some("main"), "/src/app", None)],
        );
        let git = asked(Walk::Whole);
        assert!(read_refs(&git, &tree).is_empty());
        assert!(git.walks.lock().unwrap().is_empty(), "git was not asked");

        // Its working trees are still walked. Whether a checkout is holding work is a
        // different question from what its branch tracks, and the refs going unread is no
        // reason to leave every row of the repository reading `working tree not asked`.
        assert_eq!(
            read_working_trees(&git, &tree).get("/src/app"),
            Some(&Ok(true))
        );
    }

    #[test]
    fn a_second_walk_that_dropped_a_ref_or_would_not_answer_is_quoted_on_one_line() {
        // A walk missing a ref is not this repository's refs, here as in the picker; and
        // git's words are folded, because a sentence that keeps its newlines indents its
        // tail under this repository like a checkout of its own.
        let tree = one_repo(Refs::Read, vec![worktree(Some("main"), "/src/app", None)]);
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
        // The other way this walk goes wrong, and the one the page has a line of its own
        // for. The tree read these refs — the picker is drawing `gone` from that read — so
        // saying `refs unreadable` would make the picker look wrong where it is right;
        // `refs unreadable on the second read:` is the sentence, and this is where its
        // words come from.
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
        }
    }

    fn page(tree: &Tree, refs: &RefsByRepo) -> String {
        report(
            &snapshot(),
            &Chrome::default(),
            tree,
            refs,
            &WorkingTrees::new(),
        )
    }

    #[test]
    fn a_ref_at_the_same_checkout_spelled_with_a_trailing_slash_still_matches() {
        // git and herdr spell a path differently at the edge, and the tree matched them
        // through `normalize_path`; the page has to, or it prints `none` for a branch that
        // has an upstream — on the page whose job is telling those two apart.
        let tree = one_repo(Refs::Read, vec![worktree(Some("main"), "/src/app", None)]);
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
        // The tree read the refs once — the picker is drawing `gone` from that read — and
        // only this page's own second read failed. Saying `track not read` there would make
        // the picker look wrong when it is right, which is the one thing a troubleshooting
        // page must not do.
        let tree = one_repo(
            Refs::Read,
            vec![
                worktree(Some("fix/crash"), "/wt/fix-crash", Some(Track::Gone)),
                worktree(Some("main"), "/src/app", None),
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
            page.contains("      upstream not read  track level or none  working tree"),
            "and without the upstream an empty marker cannot say which: {page}"
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
        // lookup that accepted them would hand the first ref in the list to every checkout,
        // so a page whose one job is telling upstreams apart would print another branch's.
        let tree = one_repo(Refs::Read, vec![worktree(Some("main"), "/src/app", None)]);
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
        // print that other checkout's upstream here and say nothing was wrong — on the page
        // that exists to show git and herdr disagreeing.
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
                "      no ref at this checkout for feat/login  track level or none  working tree"
            ),
            "got:\n{}",
            page(&tree, &refs)
        );
    }

    #[test]
    fn a_checkout_git_lists_no_ref_at_is_not_called_a_branch_nobody_pushed() {
        // herdr says the checkout is on `feat/login`; git's list has no ref there. The
        // words `upstream none` are what a never-pushed branch reads as, and this is not
        // that: it is git and herdr disagreeing, which is the thing to go and look at.
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
                "      no ref at this checkout for feat/login  track level or none  working tree"
            ),
            "got:\n{}",
            page(&tree, &refs)
        );
    }

    #[test]
    fn a_checkout_two_refs_claim_names_each_with_what_git_said_about_it() {
        // `domain::tree` answers a checkout that two of a repository's refs name with
        // nothing, so the picker draws no marker. Saying `level` here would turn a
        // contradiction into a measurement. The names alone would still leave a reader with
        // two branches and no way to tell which is the stale one; the upstreams do not.
        let tree = one_repo(
            Refs::Read,
            vec![worktree(Some("feat/login"), "/wt/shared", None)],
        );
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
                "      more than one ref at this checkout: \
                 feat/login \u{2192} origin/feat/login level, \
                 chore/deps \u{2192} origin/chore/deps gone  \
                 track not known  working tree"
            ),
            "got:\n{}",
            page(&tree, &refs)
        );
    }

    #[test]
    fn a_ref_with_no_upstream_among_the_others_says_so_twice_over() {
        // Three of them, and the last has never had an upstream configured. Both halves of
        // its entry read `none`, meaning two different things: git named no upstream, and
        // git had nothing to report about where it stands against the one it would push to.
        // The single-ref line keeps them apart with the words `upstream` and `track`; this
        // one has dropped the labels, which is what makes it worth pinning.
        let tree = one_repo(
            Refs::Read,
            vec![worktree(Some("feat/login"), "/wt/shared", None)],
        );
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
                 track not known  working tree"
            ),
            "got:\n{}",
            page(&tree, &refs)
        );
    }

    #[test]
    fn two_refs_at_a_checkout_the_picker_did_mark_keep_the_marker_it_drew() {
        // The picker's walk and this page's second walk are two walks, so this one can see
        // a second ref where that one saw only the marked ref. `not known` is what an empty
        // marker means here; it is not a word to put over a marker the picker is drawing.
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
                "      more than one ref at this checkout: \
                 feat/login \u{2192} origin/feat/login level, \
                 chore/deps \u{2192} origin/chore/deps level  \
                 track gone  working tree"
            ),
            "got:\n{}",
            page(&tree, &refs)
        );
    }

    #[test]
    fn a_checkout_with_nothing_out_names_the_refs_git_still_has_at_its_path() {
        // `detached` and nothing more was the whole of what this page said about such a
        // row. Named the same way the multi-ref line names them, because it is the same
        // fact — and named without saying what it means, because on this row the page
        // cannot know: see `a_row_with_a_branch_out_also_reads_as_detached`.
        let tree = one_repo(Refs::Read, vec![worktree(None, "/wt/shared", None)]);
        let mut deps = local("chore/deps", "/wt/shared", Some("origin/chore/deps"));
        deps.track = Some(Track::Gone);
        let refs = RefsByRepo::from([("/src/app".to_string(), Ok(vec![deps]))]);
        assert!(
            page(&tree, &refs).contains(
                "      detached  git names at this path: \
                 chore/deps \u{2192} origin/chore/deps gone  working tree"
            ),
            "got:\n{}",
            page(&tree, &refs)
        );
    }

    #[test]
    fn a_marker_on_a_checkout_with_nothing_out_is_on_the_page_too() {
        // The row `domain::tree::build` makes for a pane herdr did not list keeps its
        // track, so the picker draws `gone` beside a directory name about a branch nothing
        // names — issue #49. The page said `detached` and left the reader with no way to
        // ask about the marker they were looking at.
        let tree = one_repo(
            Refs::Read,
            vec![worktree(None, "/wt/shared", Some(Track::Gone))],
        );
        let refs = RefsByRepo::from([("/src/app".to_string(), Ok(Vec::new()))]);
        assert!(
            page(&tree, &refs).contains("      detached  track gone  working tree"),
            "got:\n{}",
            page(&tree, &refs)
        );
    }

    #[test]
    fn a_checkout_with_nothing_out_under_refs_that_were_not_read_says_so() {
        // No `upstream not read`, which is what a row with a branch reads: there is no
        // branch here for an upstream to be about. What is missing is the ref list, and
        // that is what it says.
        let tree = one_repo(
            Refs::Unreadable("fatal: bad ref (`git for-each-ref …`)".into()),
            vec![worktree(None, "/wt/shared", None)],
        );
        assert!(
            page(&tree, &RefsByRepo::new()).contains("      detached  refs not read  working tree"),
            "got:\n{}",
            page(&tree, &RefsByRepo::new())
        );
    }

    #[test]
    fn a_checkout_with_nothing_out_says_which_read_of_the_refs_failed() {
        // The picker read them once and drew from that; only this page's own second read
        // failed, and the repository line above says exactly that. Saying `refs not read`
        // here — which is what a repository nobody could read at all reads as, in the test
        // above — puts the row three lines under a header that contradicts it.
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
            page.contains("      detached  refs not read on the second read  working tree"),
            "got:\n{page}"
        );
    }

    #[test]
    fn a_row_with_a_branch_out_also_reads_as_detached() {
        // What `build` makes for a pane in a checkout herdr never listed: `branch: None`
        // hard-coded, the track copied from git. Nothing says the checkout is branchless —
        // `git worktree add` outside herdr leaves a branch out there — and git says as much
        // on the same line. The page cannot tell this row from the one herdr listed with
        // nothing out, so the ref list here is named and not explained; reading it as stale
        // registrations to clear would send a reader to prune a live worktree. Issue #28
        // carries the shortage, #49 the marker half.
        let tree = one_repo(
            Refs::Read,
            vec![worktree(
                None,
                "/wt/feature",
                Some(Track::Ahead(NonZeroU32::new(1).unwrap())),
            )],
        );
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
                "      detached  git names at this path: \
                 feat/login \u{2192} origin/feat/login level  track \u{2191}1  working tree"
            ),
            "got:\n{}",
            page(&tree, &refs)
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
        let page = page(&tree, &RefsByRepo::new());
        assert!(
            page.ends_with("\nnot in any repository:\n      w9:p9\n"),
            "got:\n{page}"
        );
    }
}
