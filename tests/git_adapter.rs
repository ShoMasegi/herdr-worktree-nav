//! `GitCli` against real repositories.
//!
//! These are the only tests that run git for real. Everything they cover is a place where a
//! plausible-looking implementation is quietly wrong: a bare `.git` from the repository root,
//! a branch whose name contains a slash, a linked worktree that must resolve to the same
//! repository as its parent, and a fetch that has to leave a usable base behind.

use std::path::Path;
use std::process::Command;

use herdr_gh_nav::adapter::GitCli;
use herdr_gh_nav::port::{GitPort, RefKind};
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
    git(path, &["init", "--initial-branch=main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "Test"]);
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

    let refs = GitCli.local_refs(&root).unwrap();
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
    let refs = GitCli.local_refs(&root).unwrap();
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
            "git@github.com:ShoMasegi/herdr-gh-nav.git",
        ],
    );
    assert_eq!(
        GitCli.github_slug(&root).unwrap().as_deref(),
        Some("ShoMasegi/herdr-gh-nav")
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
