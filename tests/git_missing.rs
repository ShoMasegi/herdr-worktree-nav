//! What the adapter says when there is no `git` to run.
//!
//! A binary of its own, and one test in it, because it works by emptying `PATH` for the
//! whole process: `std::env::set_var` is process-wide, and cargo runs the tests of one
//! binary on threads that share it. Everything else here needs a real git.
//!
//! The failure is the one `docs/en/usage.md` names — a `git` that is not on the path herdr
//! launched the plugin with, which fails for every checkout at once — and the only one whose
//! words come from the OS rather than from git.

use herdr_worktree_nav::adapter::GitCli;
use herdr_worktree_nav::port::GitPort;

#[test]
fn a_git_that_cannot_be_started_reaches_the_prompt_line_words_first() {
    // The sentence has to read the way a refusal does, because it arrives where a refusal
    // does: `domain::rows::refs_trouble` puts it after `me/app: refs unreadable:` and the
    // prompt line cuts what does not fit off the right. Built the other way round, what
    // survived the cut was the plugin's own argv.
    std::env::set_var("PATH", "");

    let error = GitCli
        .local_refs("/src/app")
        .expect_err("there is no git to answer");
    let words = format!("{error:#}");
    assert!(
        words.starts_with("git could not be run: "),
        "the words first: {words}"
    );
    assert!(
        words.ends_with("refs/heads refs/remotes`)"),
        "the call after them, and whole: {words}"
    );
}
