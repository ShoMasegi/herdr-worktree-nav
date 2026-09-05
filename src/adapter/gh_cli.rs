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
    GhPort, PullRequest, PullRequestOutcome, SettledPullRequest, SettledPullRequests,
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
/// Two of these are load-bearing and were both wrong once. There is **no `-R`**: that flag
/// takes `[HOST/]OWNER/REPO` and rejects a filesystem path outright, so passing `repo_root`
/// to it made every call fail — `current_dir` is what selects the repository. And the state
/// is **`closed`, not `all`**: `gh` counts open pull requests against the same window and
/// this would only throw them away, so asking for everything buys nothing and spends the
/// window on the answers it is going to discard. `closed` covers merged, which is the case
/// the sweep is mostly about.
fn settled_arguments(limit: &str) -> [&str; 8] {
    [
        "pr",
        "list",
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

pub struct GhCli;

impl GhPort for GhCli {
    fn pull_requests(&self, repo_root: &str) -> Vec<PullRequest> {
        let Ok(output) = Command::new("gh")
            .args([
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                LIMIT,
                "--json",
                "number,title,headRefName,isDraft",
            ])
            .current_dir(repo_root)
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

    fn settled_pull_requests(&self, repo_root: &str) -> Result<SettledPullRequests, String> {
        let limit = SETTLED_LIMIT.to_string();
        let output = Command::new("gh")
            .args(settled_arguments(&limit))
            .current_dir(repo_root)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("gh could not be run: {error}"))?;
        if !output.status.success() {
            // `gh`'s own words, because the alternative is a picker that says a sweep could
            // not look and cannot say why. Trimmed to the first line: the rest is usually a
            // usage dump, and this ends up on one prompt line.
            let said = String::from_utf8_lossy(&output.stderr);
            let first = said.lines().next().unwrap_or("").trim();
            return Err(if first.is_empty() {
                "gh would not answer".to_string()
            } else {
                format!("gh: {first}")
            });
        }
        // Output this cannot read is not "nothing is merged" either. It means `gh` is
        // answering in a shape this does not know, which is the same not-knowing as `gh`
        // being absent.
        let parsed: Vec<GhSettled> = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("gh answered in a shape this does not know: {error}"))?;
        // `gh` gives no sign that it truncated, so a full window is the only evidence there
        // is that there may be more behind it.
        let mut complete = parsed.len() < SETTLED_LIMIT;
        let mut pull_requests = Vec::with_capacity(parsed.len());
        for pull_request in parsed {
            let outcome = match pull_request.state {
                GhState::Merged => PullRequestOutcome::Merged,
                GhState::Closed => PullRequestOutcome::Closed,
                // A state a newer `gh` introduces. Dropping it is right — an unknown state
                // is no reason to delete anything — but the rest is then not the whole
                // answer, and saying it was would turn "something here could not be read"
                // into "there is nothing here".
                GhState::Other => {
                    complete = false;
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
        Ok(SettledPullRequests {
            pull_requests,
            complete,
        })
    }
}

/// How long a caller should wait for `gh` before giving up on the annotation.
pub const GH_BUDGET: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repository_is_chosen_by_the_directory_and_never_by_a_flag() {
        // `gh -R` takes `[HOST/]OWNER/REPO` and rejects a path outright, so a `-R repo_root`
        // here fails every call on every machine — and the failure is a `Vec::new()` that
        // reads as "no pull requests" or an `Err` that reads as "your gh is broken". A whole
        // green suite did not notice, because nothing else in it runs `gh` at all. Pinning
        // the argument list is the cheapest thing that would have.
        let arguments = settled_arguments("300");
        assert!(
            !arguments.contains(&"-R") && !arguments.contains(&"--repo"),
            "the repository comes from current_dir: {arguments:?}"
        );
        assert_eq!(
            arguments,
            [
                "pr",
                "list",
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
        let arguments = settled_arguments("300");
        let state = arguments
            .iter()
            .position(|argument| *argument == "--state")
            .map(|at| arguments[at + 1]);
        assert_eq!(state, Some("closed"));
    }
}
