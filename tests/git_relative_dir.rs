//! The adapter handed a relative path to a repository git does not take for one.
//!
//! A binary of its own, and one test in it, for the reason `git_locale.rs` is: it works by
//! changing the working directory of the whole process, and cargo runs the tests of one
//! binary on threads that share it.

use std::path::Path;
use std::process::Command;

use herdr_worktree_nav::adapter::GitCli;
use herdr_worktree_nav::port::GitPort;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_relative_path_to_a_repository_git_does_not_take_for_one_is_not_answered_for_by_the_one_around_it(
) {
    // `tests/git_adapter.rs` has the same with the path as it is and through a link.
    let outer = tempfile::tempdir().unwrap();
    git(outer.path(), &["init", "--initial-branch=main"]);
    git(outer.path(), &["commit", "--allow-empty", "-m", "outer"]);
    git(outer.path(), &["branch", "merged"]);
    let around = tempfile::tempdir().unwrap();
    let outer_checkout = around.path().join("outer-checkout");
    let outer_checkout_str = outer_checkout.to_str().unwrap();
    git(
        outer.path(),
        &["worktree", "add", "--detach", outer_checkout_str],
    );
    let inner = outer.path().join("inner");
    std::fs::create_dir(&inner).unwrap();
    git(&inner, &["init", "--initial-branch=main"]);
    git(&inner, &["commit", "--allow-empty", "-m", "inner"]);
    std::fs::write(inner.join(".git").join("HEAD"), "").unwrap();
    std::env::set_current_dir(&inner).unwrap();

    assert!(GitCli.local_refs(".").is_err(), "local_refs");
    assert!(GitCli.worktree_heads(".").is_err(), "worktree_heads");
    assert!(
        GitCli.delete_branch(".", "merged").is_err(),
        "delete_branch"
    );
    assert!(
        GitCli.remove_worktree(".", outer_checkout_str).is_err(),
        "remove_worktree"
    );
    assert!(
        outer_checkout.exists(),
        "the outer repository's checkout is still there"
    );
    git(
        outer.path(),
        &["rev-parse", "--verify", "refs/heads/merged"],
    );
}
