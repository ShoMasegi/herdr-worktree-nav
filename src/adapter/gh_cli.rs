//! `GhPort` backed by the `gh` command line.
//!
//! Mostly decoration: it annotates branches with their pull request, and every failure path
//! — `gh` not installed, not authenticated, no network, a repository GitHub has never heard
//! of — degrades to "no pull requests" rather than an error, because the picker has to keep
//! working offline.
//!
//! The sweep's question is the exception, and only in one respect: it still never fails, but
//! it says "could not ask" instead of "nothing to report". See `GhPort::settled_pull_requests`
//! and `docs/adr/0011-what-may-be-swept.md`.

use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use crate::port::{
    GhPort, GhRepo, PullRequest, PullRequestOutcome, SettledPullRequest, SettledPullRequests,
};

/// More open pull requests than anyone scrolls through in a branch picker.
const LIMIT: &str = "100";

/// A wider window than the open list, because it looks back over everything that has landed
/// rather than at what is in flight. A repository busier than this is not told "no pull
/// request" for the branches beyond it — see `SettledPullRequests::complete`.
const SETTLED_LIMIT: usize = 300;

/// The arguments that pick out the finished pull requests, as one value so the shape can be
/// read and tested rather than inferred from a builder.
///
/// Two of these are load-bearing and both were wrong once.
///
/// `-R` takes `[HOST/]OWNER/REPO`, so passing a filesystem path to it failed every call —
/// but dropping it is not the fix. Without `-R`, `gh` picks a base repository out of the
/// checkout's remotes, and for a fork that is the *parent*: it answers about a repository
/// the user does not own, with a zero exit and nothing to say so. The slug is what pins it.
///
/// The state is **`closed`, not `all`**: `gh` counts open pull requests against the same
/// window and this throws them away, so asking for everything spends the window on the
/// answers it is going to discard. `closed` covers merged, which is the case the sweep is
/// mostly about.
fn settled_arguments<'a>(slug: &'a str, limit: &'a str) -> [&'a str; 10] {
    [
        "pr",
        "list",
        "-R",
        slug,
        "--state",
        "closed",
        "--limit",
        limit,
        "--json",
        "number,headRefName,isCrossRepository,state",
    ]
}

/// What `gh` prints for `--json state`. Anything that is not one of these is a state this
/// does not know, and an unknown state is not a licence to offer a deletion.
#[derive(Deserialize, PartialEq)]
enum GhState {
    #[serde(rename = "MERGED")]
    Merged,
    #[serde(rename = "CLOSED")]
    Closed,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct GhSettled {
    number: u64,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "isCrossRepository")]
    is_cross_repository: bool,
    state: GhState,
}

#[derive(Deserialize)]
struct GhPullRequest {
    number: u64,
    title: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
}

/// Turn what `gh` printed into what the sweep decides on.
///
/// Separate from running the command because this is where everything that can be wrong
/// lives — whether an answer is the whole answer, and whether a branch name means anything
/// here — and none of that needs a network, a token, or a `gh` on the machine to test.
fn read_settled(stdout: &[u8], limit: usize) -> Result<SettledPullRequests, String> {
    // Output this cannot read is not "nothing is merged" either. It means `gh` is answering
    // in a shape this does not know, which is the same not-knowing as `gh` being absent.
    let parsed: Vec<GhSettled> = serde_json::from_slice(stdout)
        .map_err(|error| format!("gh answered in a shape this does not know: {error}"))?;
    // `gh` gives no sign that it truncated, so a full window is the only evidence there is
    // that there may be more behind it.
    let mut whole = parsed.len() < limit;
    let mut pull_requests = Vec::with_capacity(parsed.len());
    for pull_request in parsed {
        let outcome = match pull_request.state {
            GhState::Merged => PullRequestOutcome::Merged,
            GhState::Closed => PullRequestOutcome::Closed,
            // A state a newer `gh` introduces. Dropping it is right — an unknown state is no
            // reason to delete anything — but the rest is then not the whole answer, and
            // saying it was would turn "something here could not be read" into "there is
            // nothing here".
            GhState::Other => {
                whole = false;
                continue;
            }
        };
        pull_requests.push(SettledPullRequest {
            number: pull_request.number,
            head_ref: pull_request.head_ref_name,
            from_a_fork: pull_request.is_cross_repository,
            outcome,
        });
    }
    Ok(if whole {
        SettledPullRequests::All(pull_requests)
    } else {
        SettledPullRequests::Window(pull_requests)
    })
}

pub struct GhCli;

impl GhPort for GhCli {
    fn pull_requests(&self, repo: GhRepo) -> Vec<PullRequest> {
        let Ok(output) = Command::new("gh")
            .args([
                "-R",
                repo.slug,
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                LIMIT,
                "--json",
                "number,title,headRefName,isDraft",
            ])
            .current_dir(repo.root)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        let parsed: Vec<GhPullRequest> = serde_json::from_slice(&output.stdout).unwrap_or_default();
        parsed
            .into_iter()
            .map(|pr| PullRequest {
                number: pr.number,
                title: pr.title,
                head_ref: pr.head_ref_name,
                is_draft: pr.is_draft,
            })
            .collect()
    }

    fn settled_pull_requests(&self, repo: GhRepo) -> Result<SettledPullRequests, String> {
        let limit = SETTLED_LIMIT.to_string();
        let output = Command::new("gh")
            .args(settled_arguments(repo.slug, &limit))
            .current_dir(repo.root)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("gh could not be run: {error}"))?;
        if !output.status.success() {
            // `gh`'s own words, because the alternative is a picker that says a sweep could
            // not look and cannot say why. Trimmed to the first line with anything on it —
            // `gh` prefixes warnings and blank lines of its own, and the first line being one
            // of those is exactly when the reason exists and is on the second. The rest is
            // usually a usage dump, and this ends up on one prompt line.
            let said = String::from_utf8_lossy(&output.stderr);
            let reason = said.lines().map(str::trim).find(|line| !line.is_empty());
            return Err(match reason {
                // Not "gh: …". The two bugs this call has already had were malformed argv on
                // this side, and both read to a user as "your gh is broken" — which is the
                // one thing this cannot tell apart from GitHub saying no.
                Some(reason) => format!("gh refused the question this asked: {reason}"),
                None => format!("gh would not answer ({})", output.status),
            });
        }
        read_settled(&output.stdout, SETTLED_LIMIT)
    }
}

/// How long a caller should wait for `gh` before giving up on the annotation.
pub const GH_BUDGET: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repository_is_named_rather_than_guessed_from_the_remotes() {
        // Twice wrong here. `-R` was given a filesystem path, which it rejects outright, so
        // every call failed on every machine. Dropping the flag fixed that and introduced a
        // quieter fault: `gh` then picks a base repository out of the remotes, and for a
        // fork it picks the parent — answering about somebody else's repository with a zero
        // exit. A whole green suite noticed neither, because nothing else in it runs `gh`.
        // Pinning the argument list is the cheapest thing that would have.
        let arguments = settled_arguments("me/app", "300");
        assert!(
            arguments.windows(2).any(|pair| pair == ["-R", "me/app"]),
            "the repository is named, not guessed from the remotes: {arguments:?}"
        );
        assert_eq!(
            arguments,
            [
                "pr",
                "list",
                "-R",
                "me/app",
                "--state",
                "closed",
                "--limit",
                "300",
                "--json",
                "number,headRefName,isCrossRepository,state",
            ]
        );
    }

    #[test]
    fn closed_is_asked_for_rather_than_everything() {
        // `--state all` returns open pull requests too, counted against the same window and
        // then thrown away here — so a busy repository spends its whole window on the
        // answers this does not want and truncates away the ones it does.
        let arguments = settled_arguments("me/app", "300");
        let state = arguments
            .iter()
            .position(|argument| *argument == "--state")
            .map(|at| arguments[at + 1]);
        assert_eq!(state, Some("closed"));
    }
}

#[cfg(test)]
mod reading {
    use super::*;

    fn one(number: u64, head_ref: &str, state: &str, cross: bool) -> String {
        format!(
            r#"{{"number":{number},"headRefName":"{head_ref}","isCrossRepository":{cross},"state":"{state}"}}"#
        )
    }

    fn json(entries: &[String]) -> Vec<u8> {
        format!("[{}]", entries.join(",")).into_bytes()
    }

    /// The list, whichever variant it arrived in, for the assertions about its contents.
    /// Which variant it is gets asserted on its own, because that is the other half.
    fn listed(read: &SettledPullRequests) -> &[SettledPullRequest] {
        match read {
            SettledPullRequests::All(list) | SettledPullRequests::Window(list) => list,
        }
    }

    #[test]
    fn both_settled_states_come_through_as_themselves() {
        let read = read_settled(
            &json(&[
                one(1, "feat/login", "MERGED", false),
                one(2, "fix/crash", "CLOSED", false),
            ]),
            300,
        )
        .unwrap();
        // Every field, because the row the sweep draws is made of all of them: the wrong
        // number names the wrong pull request, and an empty head ref matches no branch at
        // all — which reads as "nothing to sweep here" rather than as a bug.
        assert_eq!(
            listed(&read),
            [
                SettledPullRequest {
                    number: 1,
                    head_ref: "feat/login".into(),
                    from_a_fork: false,
                    outcome: PullRequestOutcome::Merged,
                },
                SettledPullRequest {
                    number: 2,
                    head_ref: "fix/crash".into(),
                    from_a_fork: false,
                    outcome: PullRequestOutcome::Closed,
                },
            ]
        );
        assert!(
            matches!(read, SettledPullRequests::All(_)),
            "two of a window of three hundred, so this is all of them"
        );
    }

    #[test]
    fn a_branch_on_somebody_elses_fork_says_so() {
        let read = read_settled(&json(&[one(1, "patch-1", "MERGED", true)]), 300).unwrap();
        assert!(listed(&read)[0].from_a_fork);
        assert_eq!(
            listed(&read)[0].head_ref,
            "patch-1",
            "and the name is the fork's, which is the whole reason it must be told apart"
        );
    }

    #[test]
    fn a_full_window_is_not_reported_as_the_whole_answer() {
        // `gh` returns exactly the limit and says nothing about what it cut off, so this is
        // the only evidence there is. A branch missing from a list that stopped early is
        // one this could not see, not one with no pull request.
        let entries: Vec<String> = (0..3)
            .map(|n| one(n, &format!("feat/{n}"), "MERGED", false))
            .collect();
        assert!(matches!(
            read_settled(&json(&entries), 3).unwrap(),
            SettledPullRequests::Window(_)
        ));
        assert!(matches!(
            read_settled(&json(&entries), 4).unwrap(),
            SettledPullRequests::All(_)
        ));
    }

    #[test]
    fn a_state_this_does_not_know_is_dropped_and_admitted_to() {
        // Dropping is right — it is no reason to delete anything. Reporting the rest as the
        // whole answer would turn "something here could not be read" into "there is nothing
        // here", which is the one direction that matters.
        let read = read_settled(
            &json(&[
                one(1, "feat/login", "MERGED", false),
                one(2, "feat/next", "SOMETHING_NEWER", false),
            ]),
            300,
        )
        .unwrap();
        assert_eq!(listed(&read).len(), 1, "the unknown one is not guessed at");
        assert!(
            matches!(read, SettledPullRequests::Window(_)),
            "and its absence is not passed off as an answer"
        );
    }

    #[test]
    fn output_this_cannot_read_is_not_an_empty_answer() {
        assert!(read_settled(b"not json at all", 300).is_err());
        assert!(
            listed(&read_settled(b"[]", 300).unwrap()).is_empty(),
            "but an empty list is one"
        );
    }
}
