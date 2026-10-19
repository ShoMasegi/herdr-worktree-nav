//! What a git refusal reads as when a trace variable is set in the environment herdr
//! launched the plugin in.
//!
//! A binary of its own, and one test in it, because it works by setting trace variables for
//! the whole process: `std::env::set_var` is process-wide, and cargo runs the tests of one
//! binary on threads that share it.

use std::path::Path;
use std::process::Command;

use herdr_worktree_nav::adapter::GitCli;
use herdr_worktree_nav::port::GitPort;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn a_refusal_under_a_trace_variable_is_gits_words_and_not_the_trace() {
    // Issue #39. Each of these writes a timestamped line per call to stderr ahead of git's
    // own words, and `GIT_TRACE2` repeats the error itself.
    for variable in [
        "GIT_TRACE",
        "GIT_TRACE2",
        "GIT_TRACE_SETUP",
        "GIT_TRACE_PERFORMANCE",
    ] {
        std::env::set_var(variable, "1");
    }
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "--initial-branch=main"]);
    let root = repo.path().to_str().unwrap();

    let error = GitCli
        .delete_branch(root, "nope")
        .expect_err("there is no such branch to delete");
    let words = format!("{error:#}");
    assert_eq!(
        words, "error: branch 'nope' not found (`git branch -d nope`)",
        "git's words, alone, then the call"
    );
}
