//! `GhCli` against a `gh` on `PATH` that this test put there.
//!
//! Nothing in CI runs the real `gh`, so every other test of this adapter stops at the
//! command it builds and the answer it reads. This one starts the process, because two
//! things live only there: the budget a `gh` that never exits is given, and the `stderr`
//! redirection that carries `gh`'s own words to the user. The test binary is its own
//! process, so the `PATH` it changes is nobody else's — the same rule `git_locale.rs` is
//! under for `LC_ALL`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use herdr_worktree_nav::adapter::{GhCli, GH_BUDGET};
use herdr_worktree_nav::port::{GhPort, SettledPullRequests, Slug};

/// A `gh` whose behaviour the test picks by writing one word into a file beside it.
fn fake_gh(dir: &Path) {
    let mode = dir.join("mode");
    let script = format!(
        "#!/bin/sh\ncase \"$(cat {mode})\" in\n  hang) echo $$ > {pid}; exec sleep 60 ;;\n  refuse) echo \"gh: the test refused\" >&2; exit 1 ;;\n  answer) printf '%s' '[{{\"number\":7,\"headRefName\":\"feat/x\",\"isCrossRepository\":false,\"state\":\"MERGED\"}}]' ;;\n  open) printf '%s' '[{{\"number\":12,\"title\":\"a title\",\"headRefName\":\"feat/y\",\"isDraft\":true}}]' ;;\n  wordy) sleep 1; yes ' ' | head -c 262144; printf '%s' '[{{\"number\":7,\"headRefName\":\"feat/x\",\"isCrossRepository\":false,\"state\":\"MERGED\"}}]' ;;\nesac\n",
        mode = mode.display(),
        pid = dir.join("pid").display()
    );
    let gh = dir.join("gh");
    std::fs::write(&gh, script).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `PATH` with `dir` in front, put back when the test ends however it ends.
struct PathWith {
    was: Option<std::ffi::OsString>,
}

impl PathWith {
    fn front(dir: &Path) -> Self {
        let was = std::env::var_os("PATH");
        let rest = was.clone().unwrap_or_default();
        let mut path = dir.as_os_str().to_os_string();
        path.push(":");
        path.push(rest);
        std::env::set_var("PATH", path);
        Self { was }
    }
}

impl Drop for PathWith {
    fn drop(&mut self) {
        match &self.was {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
    }
}

#[test]
fn a_gh_on_the_path_is_given_the_budget_and_no_more() {
    let dir = tempfile::tempdir().unwrap();
    fake_gh(dir.path());
    let _path = PathWith::front(dir.path());
    let slug = Slug::owner_repo("me", "app").unwrap();

    // A `gh` that never exits: the sweep's question is refused within the budget, with a
    // reason, rather than waited on for ever.
    std::fs::write(dir.path().join("mode"), "hang").unwrap();
    let asked = Instant::now();
    let refused = GhCli
        .settled_pull_requests(&slug)
        .expect_err("a gh that never answers is a refusal");
    let waited = asked.elapsed();
    assert_eq!(refused, "gh did not answer within 5s");
    assert!(waited >= GH_BUDGET, "it waited the budget out: {waited:?}");
    assert!(
        waited < GH_BUDGET + Duration::from_secs(3),
        "and not much longer: {waited:?}"
    );
    // And the process is gone, not left to sleep on: `kill -0` on its pid finds nothing.
    let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .status()
        .unwrap()
        .success();
    assert!(!alive, "the gh that ran out of time was killed");

    // The decoration query under the same gh: nothing, and not a wait either.
    let asked = Instant::now();
    assert!(GhCli.pull_requests(&slug).is_empty());
    assert!(asked.elapsed() < GH_BUDGET + Duration::from_secs(3));

    // A `gh` that refuses: its words reach the sentence, which is what `stderr` being piped
    // is for.
    std::fs::write(dir.path().join("mode"), "refuse").unwrap();
    assert_eq!(
        GhCli.settled_pull_requests(&slug),
        Err("gh refused the question this asked: gh: the test refused".to_string())
    );
    assert!(GhCli.pull_requests(&slug).is_empty());

    // A `gh` that answers: what it wrote on `stdout` is the answer, read to the end.
    std::fs::write(dir.path().join("mode"), "answer").unwrap();
    let answered = GhCli
        .settled_pull_requests(&slug)
        .expect("an answer within the budget");
    assert_eq!(numbers(&answered), [7]);

    // The decoration query's answer, which is the half of this adapter that ships today and
    // whose every failure is the same empty list.
    std::fs::write(dir.path().join("mode"), "open").unwrap();
    let decorated = GhCli.pull_requests(&slug);
    let seen: Vec<(u64, &str, bool)> = decorated
        .iter()
        .map(|pull_request| {
            (
                pull_request.number,
                pull_request.head_ref.as_str(),
                pull_request.is_draft,
            )
        })
        .collect();
    assert_eq!(seen, [(12, "feat/y", true)]);

    // A `gh` that is slow and says more than a pipe holds: an answer, not a budget spent.
    // Reading the pipes after the wait rather than on threads is what this catches — the
    // process blocks on a full pipe, and the wait then sits there until the deadline.
    std::fs::write(dir.path().join("mode"), "wordy").unwrap();
    let asked = Instant::now();
    let wordy = GhCli
        .settled_pull_requests(&slug)
        .expect("a gh that finishes inside the budget is an answer");
    let waited = asked.elapsed();
    assert!(
        waited < GH_BUDGET,
        "it did not wait the budget out: {waited:?}"
    );
    assert_eq!(numbers(&wordy), [7]);
}

/// The pull request numbers of an answer, in the order `gh` listed them.
fn numbers(answered: &SettledPullRequests) -> Vec<u64> {
    answered
        .pull_requests()
        .iter()
        .map(|pull_request| pull_request.number)
        .collect()
}
