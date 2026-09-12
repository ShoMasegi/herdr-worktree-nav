//! `GitCli` against real repositories.
//!
//! These are the only tests that run git for real. Everything they cover is a place where a
//! plausible-looking implementation is quietly wrong: a bare `.git` from the repository root,
//! a branch whose name contains a slash, a linked worktree that must resolve to the same
//! repository as its parent, and a fetch that has to leave a usable base behind.

use std::num::NonZeroU32;
use std::path::Path;
use std::process::Command;

use herdr_worktree_nav::adapter::GitCli;
use herdr_worktree_nav::port::{GitPort, RefKind, Slug, Track};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository with one commit on `main`, an extra `feat/login` branch, and a committer
/// configured so commits work on a machine with no global git identity.
fn repository() -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path();
    // The ref format is pinned rather than inherited, for the reason `push.default` is
    // below: two tests here reach into `.git/refs` and `.git/packed-refs` directly, and
    // under a global `init.defaultRefFormat = reftable` neither file exists, so both fail
    // on a machine whose git is configured that way while the other twenty pass. `-c` on
    // the command rather than `--ref-format=`, which git 2.43 on CI does not know: an
    // unknown configuration key is ignored where an unknown flag is fatal.
    git(
        path,
        &[
            "-c",
            "init.defaultRefFormat=files",
            "init",
            "--initial-branch=main",
        ],
    );
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "Test"]);
    // Pinned, not inherited. `%(push:track)` answers a different question under each
    // `push.default`, so a test that leaves it to the machine's global config is testing
    // the machine — see `a_branch_nobody_has_pushed_is_never_gone`.
    git(path, &["config", "push.default", "simple"]);
    std::fs::write(path.join("README.md"), "hello\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "first"]);
    git(path, &["branch", "feat/login"]);
    dir
}

fn path_str(path: &Path) -> String {
    // macOS reports /var while the temp dir is /private/var; canonicalise both sides so the
    // comparisons in these tests are about git, not about symlinks.
    path.canonicalize().unwrap().to_string_lossy().into_owned()
}

#[test]
fn identifies_the_repository_from_the_root_a_subdirectory_and_a_worktree() {
    let repo = repository();
    let root = path_str(repo.path());

    let from_root = GitCli
        .identify(&root)
        .unwrap()
        .expect("root is a repository");
    // The interesting part: from the root, `git rev-parse --git-common-dir` prints a bare
    // ".git" unless --path-format=absolute is passed, which would not be a usable identity.
    assert_eq!(from_root.repo_key, format!("{root}/.git"));
    assert_eq!(from_root.checkout_path, root);
    assert_eq!(from_root.branch.as_deref(), Some("main"));

    std::fs::create_dir(repo.path().join("src")).unwrap();
    let from_subdir = GitCli
        .identify(&path_str(&repo.path().join("src")))
        .unwrap()
        .expect("a subdirectory is still in the repository");
    assert_eq!(from_subdir.checkout_path, root, "resolves to the top level");

    // A linked worktree is a different checkout of the same repository, and has to share the
    // identity — that is what groups them under one heading in the picker.
    let worktree = repo.path().join("wt");
    git(
        repo.path(),
        &["worktree", "add", worktree.to_str().unwrap(), "feat/login"],
    );
    let from_worktree = GitCli
        .identify(&path_str(&worktree))
        .unwrap()
        .expect("a worktree is a repository");
    assert_eq!(from_worktree.repo_key, from_root.repo_key);
    assert_ne!(from_worktree.checkout_path, from_root.checkout_path);
    assert_eq!(from_worktree.branch.as_deref(), Some("feat/login"));
}

#[test]
fn a_path_outside_any_repository_is_an_answer_rather_than_an_error() {
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(GitCli.identify(&path_str(empty.path())).unwrap(), None);
}

#[test]
fn a_detached_checkout_reports_no_branch() {
    let repo = repository();
    let head = GitCli.head_ref(&path_str(repo.path())).unwrap();
    git(repo.path(), &["checkout", "--detach", &head]);
    let identity = GitCli.identify(&path_str(repo.path())).unwrap().unwrap();
    assert_eq!(
        identity.branch, None,
        "git prints \"HEAD\" when detached, which is not a branch name"
    );
}

#[test]
fn reads_local_and_remote_refs_without_confusing_the_two() {
    let repo = repository();
    let root = path_str(repo.path());
    // A fake remote, so this needs no network.
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "origin", "main"]);
    git(repo.path(), &["fetch", "-q", "origin"]);

    let refs = GitCli.local_refs(&root).unwrap().refs;
    let local: Vec<_> = refs
        .iter()
        .filter(|r| r.kind == RefKind::Local)
        .map(|r| r.name.as_str())
        .collect();
    // `feat/login` contains a slash, which is exactly what makes `refname:short` ambiguous
    // with a remote ref printed as `origin/main`.
    assert!(local.contains(&"main"), "got {local:?}");
    assert!(local.contains(&"feat/login"), "got {local:?}");

    let remote_refs: Vec<_> = refs
        .iter()
        .filter(|r| r.kind == RefKind::Remote)
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(
        remote_refs,
        ["main"],
        "the remote prefix is stripped, leaving the branch as it exists upstream"
    );

    assert!(
        refs.iter().any(|r| r.subject.as_deref() == Some("first")),
        "the tip commit's subject comes through"
    );
    assert!(refs.iter().all(|r| r.committed_at.is_some()));
}

#[test]
fn lists_remote_heads_and_fetches_one_into_a_usable_base() {
    let repo = repository();
    let root = path_str(repo.path());
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "origin", "main", "feat/login"]);

    let mut heads = GitCli.remote_heads(&root).unwrap();
    heads.sort();
    assert_eq!(heads, ["feat/login", "main"]);

    // `git push` writes the remote-tracking refs as a side effect, so drop them to get the
    // state this test is actually about: a branch that exists on the remote and has never
    // been fetched into this clone.
    git(
        repo.path(),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    git(
        repo.path(),
        &["update-ref", "-d", "refs/remotes/origin/feat/login"],
    );
    assert!(
        !GitCli
            .local_refs(&root)
            .unwrap()
            .refs
            .iter()
            .any(|r| r.kind == RefKind::Remote),
        "there should be no local ref to base a worktree on yet"
    );

    GitCli.fetch_branch(&root, "feat/login").unwrap();

    // The point of the fetch: `origin/feat/login` now exists, so the worktree can be cut
    // from the branch that is actually on the remote rather than from HEAD.
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "--verify", "refs/remotes/origin/feat/login"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fetch should have written the remote-tracking ref"
    );
}

#[test]
fn fetching_the_repository_updates_every_branch_and_drops_the_ones_that_are_gone() {
    let repo = repository();
    let root = path_str(repo.path());
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "origin", "main", "feat/login"]);
    git(
        repo.path(),
        &["update-ref", "-d", "refs/remotes/origin/feat/login"],
    );

    // A branch the remote has and this clone has never fetched.
    GitCli.fetch_all(&root).unwrap();
    let refs = GitCli.local_refs(&root).unwrap().refs;
    let fetched = refs
        .iter()
        .find(|r| r.name == "feat/login" && r.kind == RefKind::Remote);
    assert!(
        fetched.is_some_and(|r| r.committed_at.is_some() && r.subject.is_some()),
        "the fetch is what gives a remote branch a date and a subject: {refs:?}"
    );

    // And one it no longer has.
    let remote_repo = remote.path().to_str().unwrap().to_string();
    Command::new("git")
        .arg("--git-dir")
        .arg(&remote_repo)
        .args(["branch", "-D", "feat/login"])
        .output()
        .unwrap();

    GitCli.fetch_all(&root).unwrap();
    assert!(
        !GitCli
            .local_refs(&root)
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "feat/login" && r.kind == RefKind::Remote),
        "--prune is what keeps a deleted branch from haunting the list"
    );
}

#[test]
fn a_fetch_that_cannot_reach_the_remote_says_so_rather_than_blaming_the_repository() {
    // git exits 128 for every fatal error, so treating that code alone as "not a git
    // repository" turns an unreachable remote into a diagnosis about the wrong thing —
    // and the picker puts that message in front of the user.
    let repo = repository();
    let root = path_str(repo.path());
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "origin", "feat/login"]);
    drop(remote);

    let error = GitCli
        .fetch_branch(&root, "feat/login")
        .expect_err("the remote is gone");
    let error = format!("{error:#}");
    assert!(
        error.contains("Could not read from remote repository")
            || error.contains("does not appear to be a git repository"),
        "the failure should be git's own account of it, got: {error}"
    );
    assert!(
        !error.contains("is not a git repository, but was expected to be"),
        "the repository is fine; the remote is not: {error}"
    );
}

#[test]
fn removing_a_worktree_takes_the_checkout_and_leaves_the_branch() {
    let repo = repository();
    let root = path_str(repo.path());
    // In a temp dir of its own, so a failed run leaves nothing behind in the shared one.
    let elsewhere = tempfile::tempdir().unwrap();
    let checkout = elsewhere.path().join("gone-worktree");
    git(
        repo.path(),
        &["worktree", "add", checkout.to_str().unwrap(), "feat/login"],
    );
    assert!(checkout.join("README.md").exists());

    GitCli
        .remove_worktree(&root, checkout.to_str().unwrap())
        .unwrap();

    assert!(!checkout.exists(), "the checkout is gone from disk");
    let listed = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["worktree", "list"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&listed.stdout).contains("gone-worktree"),
        "and gone from git's record of them"
    );
    assert!(
        GitCli
            .local_refs(&root)
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "feat/login"),
        "the branch it was on is not the picker's to delete"
    );
}

#[test]
fn removing_a_worktree_with_uncommitted_work_refuses_and_says_why() {
    let repo = repository();
    let root = path_str(repo.path());
    let elsewhere = tempfile::tempdir().unwrap();
    let checkout = elsewhere.path().join("dirty-worktree");
    git(
        repo.path(),
        &["worktree", "add", checkout.to_str().unwrap(), "feat/login"],
    );
    std::fs::write(checkout.join("README.md"), "edited but not committed\n").unwrap();

    let error = GitCli
        .remove_worktree(&root, checkout.to_str().unwrap())
        .expect_err("git should refuse to throw work away");
    let error = format!("{error:#}");
    assert!(
        error.contains("contains modified or untracked files"),
        "git's own reason should reach the user: {error}"
    );
    assert!(
        checkout.join("README.md").exists(),
        "and nothing should have been deleted"
    );
}

#[test]
fn the_main_checkout_is_not_a_worktree_that_can_be_removed() {
    // The picker refuses before asking, but the adapter must not pretend otherwise either.
    let repo = repository();
    let root = path_str(repo.path());
    assert!(GitCli.remove_worktree(&root, &root).is_err());
}

#[test]
fn recognises_a_github_origin_and_ignores_anything_else() {
    let repo = repository();
    let root = path_str(repo.path());

    assert_eq!(GitCli.github_slug(&root).unwrap(), None, "no origin yet");

    git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:ShoMasegi/herdr-worktree-nav.git",
        ],
    );
    assert_eq!(
        GitCli
            .github_slug(&root)
            .unwrap()
            .as_ref()
            .map(Slug::as_str),
        Some("ShoMasegi/herdr-worktree-nav")
    );

    git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.com:owner/repo.git",
        ],
    );
    assert_eq!(
        GitCli.github_slug(&root).unwrap(),
        None,
        "a non-GitHub remote falls back to the directory name elsewhere"
    );
}

#[test]
fn head_ref_names_the_branch_and_falls_back_to_a_commit_when_detached() {
    let repo = repository();
    let root = path_str(repo.path());
    assert_eq!(GitCli.head_ref(&root).unwrap(), "main");

    let commit = GitCli.head_ref(&root).unwrap();
    git(repo.path(), &["checkout", "--detach", &commit]);
    let detached = GitCli.head_ref(&root).unwrap();
    assert_ne!(
        detached, "HEAD",
        "\"HEAD\" is not something a worktree can be based on"
    );
    assert_eq!(detached.len(), 40, "a full commit id: {detached}");
}

/// A repository with an `origin` that is a real bare repository on disk, so upstream
/// tracking is exercised without a network.
fn with_origin() -> (TempDir, TempDir) {
    let repo = repository();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "-u", "origin", "main"]);
    (repo, remote)
}

fn track_of(refs: &[herdr_worktree_nav::port::GitRef], name: &str) -> Option<Track> {
    refs.iter()
        .find(|r| r.kind == RefKind::Local && r.name == name)
        .unwrap_or_else(|| panic!("no local ref {name}"))
        .track
}

fn upstream_of(refs: &[herdr_worktree_nav::port::GitRef], name: &str) -> Option<String> {
    refs.iter()
        .find(|r| r.kind == RefKind::Local && r.name == name)
        .unwrap_or_else(|| panic!("no local ref {name}"))
        .upstream
        .clone()
}

#[test]
fn a_branch_level_with_its_upstream_has_nothing_to_report() {
    let (repo, _remote) = with_origin();
    let refs = GitCli.local_refs(&path_str(repo.path())).unwrap().refs;
    assert_eq!(track_of(&refs, "main"), None);
    // Never pushed, so it has no upstream at all — also nothing to say, and in particular
    // not "gone".
    assert_eq!(track_of(&refs, "feat/login"), None);

    // The same `None` twice, and the upstream's name is what tells them apart: `dump`
    // reads it, since a row with no marker cannot say which of the two it is.
    assert_eq!(upstream_of(&refs, "main").as_deref(), Some("origin/main"));
    assert_eq!(upstream_of(&refs, "feat/login"), None);
}

#[test]
fn ahead_and_behind_come_out_of_the_ref_walk() {
    let (repo, remote) = with_origin();
    // One commit here that origin does not have.
    std::fs::write(repo.path().join("local.txt"), "local\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "local only"]);
    assert_eq!(
        track_of(
            &GitCli.local_refs(&path_str(repo.path())).unwrap().refs,
            "main"
        ),
        Some(Track::Ahead(NonZeroU32::new(1).unwrap()))
    );

    // And one on origin that this repository does not have, made through a second clone so
    // the first one is genuinely behind.
    let other = tempfile::tempdir().unwrap();
    let other = other.path().join("clone");
    git(
        Path::new("/"),
        &[
            "clone",
            "-q",
            remote.path().to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.email", "test@example.com"]);
    git(&other, &["config", "user.name", "Test"]);
    std::fs::write(other.join("theirs.txt"), "theirs\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "theirs"]);
    git(&other, &["push", "-q", "origin", "main"]);

    git(repo.path(), &["fetch", "-q", "origin"]);
    assert_eq!(
        track_of(
            &GitCli.local_refs(&path_str(repo.path())).unwrap().refs,
            "main"
        ),
        Some(Track::Diverged {
            ahead: NonZeroU32::new(1).unwrap(),
            behind: NonZeroU32::new(1).unwrap()
        })
    );
}

#[test]
fn a_branch_nobody_has_pushed_is_never_gone() {
    // Under `push.default = current` the push destination of a never-pushed branch is a ref
    // that has never existed, and git reports that as `[gone]` in `%(push:track)`. Believing
    // it would put the finished-with marker on the newest branch in the repository — and on
    // exactly the branches a sweep could not undo deleting, since they exist nowhere else.
    let (repo, _remote) = with_origin();
    git(repo.path(), &["config", "push.default", "current"]);

    let refs = GitCli.local_refs(&path_str(repo.path())).unwrap().refs;
    assert_eq!(track_of(&refs, "feat/login"), None);
}

#[test]
fn an_upstream_deleted_on_the_remote_reads_as_gone() {
    // The ordinary end of a branch whose pull request was merged: GitHub deletes the head,
    // a pruning fetch drops the remote-tracking ref, and git starts calling it gone. It is
    // the marker the sweep will be built on.
    let (repo, remote) = with_origin();
    git(repo.path(), &["push", "-q", "-u", "origin", "feat/login"]);
    git(remote.path(), &["branch", "-D", "feat/login"]);
    git(repo.path(), &["fetch", "-q", "--prune", "origin"]);

    let refs = GitCli.local_refs(&path_str(repo.path())).unwrap().refs;
    assert_eq!(track_of(&refs, "feat/login"), Some(Track::Gone));
    assert_eq!(track_of(&refs, "main"), None, "only the deleted one");
}

#[test]
fn a_branch_says_which_checkout_has_it() {
    // This is what ties a branch to a row in the panes view without assuming that two
    // things named `feat/login` are the same one.
    let repo = repository();
    let root = path_str(repo.path());
    let worktree = repo.path().join("wt");
    git(
        repo.path(),
        &["worktree", "add", worktree.to_str().unwrap(), "feat/login"],
    );

    let refs = GitCli.local_refs(&root).unwrap().refs;
    let checked_out = |name: &str| {
        refs.iter()
            .find(|r| r.kind == RefKind::Local && r.name == name)
            .unwrap()
            .worktree_path
            .clone()
    };
    assert_eq!(checked_out("main").as_deref(), Some(root.as_str()));
    assert_eq!(
        checked_out("feat/login").map(|p| path_str(Path::new(&p))),
        Some(path_str(&worktree))
    );
}

#[test]
fn a_checkout_git_will_not_look_at_is_an_error_rather_than_a_clean_one() {
    // The whole `Unreadable` state upstream of this rests on the refusal arriving as an
    // `Err`. If it were ever softened into empty output, every checkout would be recorded
    // as clean and nothing above would notice.
    let empty = tempfile::tempdir().unwrap();
    assert!(GitCli.is_dirty(&path_str(empty.path())).is_err());
}

#[test]
fn a_ref_git_cannot_read_is_named_and_every_other_ref_is_still_listed() {
    // git drops a broken loose ref with a warning and exits 0. Read as a clean exit, that
    // is one checkout with no marker — which is what a branch with nothing to report looks
    // like — and the repository saying nothing, which is the silence issue #21 was about.
    // So the walk carries git's words. It carries the refs git could read as well: they are
    // a list of branches, and the branches view reads an empty one as "every branch here is
    // only on the remote".
    let repo = repository();
    std::fs::write(
        repo.path().join(".git/refs/heads/feat/login"),
        "not-a-sha\n",
    )
    .unwrap();

    let walk = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could and exited 0");
    let words = walk.dropped.expect("git said which ref it would not read");
    assert!(
        words.starts_with("warning: ignoring broken ref refs/heads/feat/login"),
        "git's words, and first: {words}"
    );
    assert!(
        words.ends_with("refs/heads/feat/login"),
        "git's words, and only those: the call they came from is left off this one sentence, \
         because it is always this call and its format string is most of the line: {words}"
    );
    let names: Vec<&str> = walk.refs.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"main"),
        "the refs git could read: {names:?}"
    );
    assert!(
        !names.contains(&"feat/login"),
        "and the one it could not is the one that is missing: {names:?}"
    );
}

#[test]
fn every_ref_git_dropped_is_named_and_not_just_the_first() {
    // git says it once per ref, so a repository with two broken refs gets two lines, and the
    // words carry both: a reader told about one, who fixes it and is then told about the
    // next, has been given half of what git already knew. What the prompt line can show of
    // them is a question of width — `src/ui/render.rs` measures that, and `dump` prints the
    // whole sentence either way.
    let repo = repository();
    git(repo.path(), &["branch", "chore/deps"]);
    for branch in ["feat/login", "chore/deps"] {
        std::fs::write(
            repo.path().join(format!(".git/refs/heads/{branch}")),
            "not-a-sha\n",
        )
        .unwrap();
    }

    let walk = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could and exited 0");
    let words = walk.dropped.expect("git said which refs it would not read");
    for branch in ["feat/login", "chore/deps"] {
        assert!(
            words.contains(&format!("ignoring broken ref refs/heads/{branch}")),
            "{branch} is named: {words}"
        );
    }
}

#[test]
fn a_refusal_puts_gits_words_before_the_call() {
    // The other way round, the `--format=` string put git's words 191 columns in — past
    // the right edge of every prompt line the picker draws.
    let repo = repository();
    std::fs::write(
        repo.path().join(".git/packed-refs"),
        "this is not a packed-refs file\n",
    )
    .unwrap();

    let error = GitCli
        .local_refs(&path_str(repo.path()))
        .expect_err("git cannot read the refs");
    let words = format!("{error:#}");
    assert!(words.starts_with("fatal: "), "git's words first: {words}");
    let call = words
        .find("(`git for-each-ref")
        .expect("the call is named after them");
    assert!(
        words[..call].contains("packed-refs"),
        "and what git said is all before it: {words}"
    );
}

#[test]
fn a_branch_with_no_upstream_is_still_measured_against_where_it_would_push() {
    // Which is what `%(push:track)` is in the format string for. Pushed without `-u`, so
    // there is no upstream to compare against and `%(upstream:track)` says nothing.
    let (repo, _remote) = with_origin();
    git(repo.path(), &["config", "push.default", "current"]);
    git(repo.path(), &["push", "-q", "origin", "feat/login"]);
    std::fs::write(repo.path().join("more.txt"), "more\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "one more"]);
    git(repo.path(), &["branch", "-f", "feat/login", "HEAD"]);

    assert_eq!(
        track_of(
            &GitCli.local_refs(&path_str(repo.path())).unwrap().refs,
            "feat/login"
        ),
        Some(Track::Ahead(NonZeroU32::new(1).unwrap()))
    );
}

#[test]
fn dirty_means_what_worktree_remove_means_by_it() {
    let repo = repository();
    let root = path_str(repo.path());
    assert!(
        !GitCli.is_dirty(&root).unwrap(),
        "a fresh checkout is clean"
    );

    // An untracked file counts, because it counts to `git worktree remove` — the marker has
    // to mean the same thing there as it does on `Shift-D`.
    std::fs::write(repo.path().join("scratch.txt"), "notes\n").unwrap();
    assert!(GitCli.is_dirty(&root).unwrap(), "untracked files count");

    std::fs::remove_file(repo.path().join("scratch.txt")).unwrap();
    assert!(!GitCli.is_dirty(&root).unwrap());

    std::fs::write(repo.path().join("README.md"), "changed\n").unwrap();
    assert!(GitCli.is_dirty(&root).unwrap(), "and so do modified ones");
}

/// The `%(worktreepath)` of a local ref, for the premises below.
fn worktree_path_of<'a>(
    refs: &'a [herdr_worktree_nav::port::GitRef],
    name: &str,
) -> Option<&'a str> {
    refs.iter()
        .find(|r| r.kind == RefKind::Local && r.name == name)
        .unwrap_or_else(|| panic!("no local ref {name}"))
        .worktree_path
        .as_deref()
}

/// Two worktrees, one of which now sits at the path the other's entry still names. Returns
/// that path.
///
/// The way a person reaches it: one directory moved out of the way and the other moved into
/// its place. `feat/login`'s entry names a path that `chore/deps`'s checkout is now sitting
/// in, and `chore/deps`'s own entry still names the directory it left. The test below adds
/// the one step that takes it somewhere.
fn a_checkout_at_another_entrys_path(repo: &TempDir) -> String {
    git(repo.path(), &["branch", "chore/deps"]);
    let one = repo.path().join("one");
    let two = repo.path().join("two");
    git(
        repo.path(),
        &["worktree", "add", one.to_str().unwrap(), "feat/login"],
    );
    git(
        repo.path(),
        &["worktree", "add", two.to_str().unwrap(), "chore/deps"],
    );
    let shared = path_str(&one);
    std::fs::rename(&one, repo.path().join("aside")).expect("one moves out of the way");
    std::fs::rename(&two, &one).expect("two moves into its place");
    shared
}

#[test]
fn two_repositories_can_name_one_path_and_prune_will_not_part_them() {
    // Half of what `domain::tree::tracks` is keyed by repository for, and all of why
    // `git worktree prune` is not the answer instead. git goes on naming the path a moved
    // worktree had; let another repository put a worktree there and a prune in the first
    // one leaves that entry alone, so both repositories name the path with nothing to
    // clear it. Issue #31.
    //
    // The prune below is a negative control: deleting it leaves this test green, because
    // what it asserts is that prune does nothing here.
    // `a_prune_that_does_clear_a_stale_entry` is the other half: with nothing left at the
    // path, prune does clear the entry.
    let old = repository();
    let app = repository();
    let shared = old.path().join("shared");
    git(
        old.path(),
        &["worktree", "add", shared.to_str().unwrap(), "feat/login"],
    );
    // Taken before the move: `path_str` canonicalises, and a path that is gone has none.
    let had = path_str(&shared);
    std::fs::rename(&shared, old.path().join("moved-away")).expect("the directory moves");
    git(app.path(), &["worktree", "add", &had, "feat/login"]);

    git(old.path(), &["worktree", "prune"]);

    for repo in [&old, &app] {
        let refs = GitCli
            .local_refs(&path_str(repo.path()))
            .expect("git listed what it could")
            .refs;
        assert_eq!(
            worktree_path_of(&refs, "feat/login"),
            Some(had.as_str()),
            "both repositories name it, after a prune: {:?}",
            refs
        );
    }
}

#[test]
fn a_prune_that_does_clear_a_stale_entry() {
    // The positive control for
    // `two_repositories_can_name_one_path_and_prune_will_not_part_them`: with nothing
    // sitting at the path, prune clears the entry and the ref stops naming it. Without
    // this, "prune will not part them" would be a sentence no test could tell from "prune
    // does nothing at all".
    let repo = repository();
    let gone = repo.path().join("gone");
    git(
        repo.path(),
        &["worktree", "add", gone.to_str().unwrap(), "feat/login"],
    );
    let had = path_str(&gone);
    std::fs::rename(&gone, repo.path().join("moved-away")).expect("the directory moves");
    let before = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could")
        .refs;
    assert_eq!(
        worktree_path_of(&before, "feat/login"),
        Some(had.as_str()),
        "git still names it after the move: {before:?}"
    );

    git(repo.path(), &["worktree", "prune"]);

    let refs = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could")
        .refs;
    assert_eq!(
        worktree_path_of(&refs, "feat/login"),
        None,
        "prune cleared it: {refs:?}"
    );
}

#[test]
fn one_repository_can_name_one_path_from_two_refs() {
    // The other half: the second ref `tracks` answers with nothing rather than with
    // either of them is a state git makes, not one only a broken port could hand it.
    let repo = repository();
    let shared = a_checkout_at_another_entrys_path(&repo);
    // Repair rewrites the entry that lost its directory rather than adding one, and what it
    // rewrites it to is the path the other entry already names.
    git(repo.path(), &["worktree", "repair", &shared]);

    let refs = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could")
        .refs;
    let mut naming: Vec<&str> = refs
        .iter()
        .filter(|r| r.kind == RefKind::Local && r.worktree_path.as_deref() == Some(shared.as_str()))
        .map(|r| r.name.as_str())
        .collect();
    naming.sort_unstable();
    assert_eq!(naming, ["chore/deps", "feat/login"], "refs: {:?}", refs);
}

#[test]
fn a_worktree_whose_directory_moved_is_still_listed_at_the_path_it_had() {
    // The first premise `domain::tree::tracks` is keyed by repository for: git does not
    // stop reporting a linked worktree when its directory goes away, it goes on naming the
    // path the worktree had. The tests above build what that leads to.
    let repo = repository();
    let worktree = repo.path().join("wt");
    git(
        repo.path(),
        &["worktree", "add", worktree.to_str().unwrap(), "feat/login"],
    );
    // Taken before the move: `path_str` canonicalises, and a path that is gone has none.
    let had = path_str(&worktree);
    std::fs::rename(&worktree, repo.path().join("moved-away")).expect("the directory moves");
    // Without this the assertion below is one `path_str` compared with itself: `had` was
    // taken before the move, so it holds whether or not anything moved.
    assert!(
        !worktree.exists(),
        "the directory really is gone from {had}"
    );

    let refs = GitCli.local_refs(&path_str(repo.path())).unwrap().refs;
    let listed = refs
        .iter()
        .find(|r| r.kind == RefKind::Local && r.name == "feat/login")
        .expect("git still lists the branch");
    assert_eq!(
        listed.worktree_path.as_deref(),
        Some(had.as_str()),
        "the path it had, not the one it has: {:?}",
        listed.worktree_path
    );
}

#[test]
fn a_ref_carrying_gone_can_name_a_path_whose_checkout_has_no_branch_out() {
    // Why `domain::tree::build` asks for a track only where herdr says something is checked
    // out. Two entries name one path and the checkout sitting there is then detached, so
    // one entry goes on naming a path with nothing out — while the ref that entry is for
    // carries `[gone]`, because its upstream was deleted on the remote. Nothing but the
    // guard keeps that `[gone]` off the branchless row, and `domain::sweep::judge` offers a
    // clean `gone` row for deletion by default.
    //
    // This holds git's half. The other half — that herdr reports that path as a checkout
    // with no branch — is an assumption about herdr's API, written down on
    // `port::Worktree::branch`, and there is no herdr in CI to hold it.
    let (repo, _remote) = with_origin();
    let shared = a_checkout_at_another_entrys_path(&repo);
    // Both, because which entry repair binds to the directory is git's own business and
    // not the same everywhere: whichever one is left naming the path has to be the one
    // carrying the marker. Where `feat/login` is the one left, the `chore/deps` half of
    // this push and of the delete below is inert, and deleting it leaves the test green;
    // it is here for the machines where the other one is left.
    git(
        repo.path(),
        &["push", "-q", "-u", "origin", "feat/login", "chore/deps"],
    );
    // Deleting this leaves the test green, and it still has to be here: repair is what
    // makes git list a second, branchless entry at `shared`, which is the half `build`
    // reads. `GitCli` has no way to list worktrees, so nothing below can see it — the
    // fixture is faithful to the state, and the assertions reach only the ref side of it.
    git(repo.path(), &["worktree", "repair", &shared]);
    git(
        repo.path(),
        &[
            "push",
            "-q",
            "origin",
            "--delete",
            "feat/login",
            "chore/deps",
        ],
    );
    git(Path::new(&shared), &["checkout", "--detach"]);

    let identity = GitCli
        .identify(&shared)
        .expect("git answered")
        .expect("a repository");
    assert_eq!(
        identity.branch, None,
        "nothing is checked out at {shared} any more"
    );

    let refs = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could")
        .refs;
    let naming: Vec<(&str, Option<Track>)> = refs
        .iter()
        .filter(|r| r.kind == RefKind::Local && r.worktree_path.as_deref() == Some(shared.as_str()))
        .map(|r| (r.name.as_str(), r.track))
        .collect();
    assert_eq!(naming.len(), 1, "one entry goes on naming it: {refs:?}");
    assert_eq!(
        naming[0].1,
        Some(Track::Gone),
        "and the ref it is for carries a marker: {refs:?}"
    );
}
