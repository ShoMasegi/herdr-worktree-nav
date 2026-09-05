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

use crate::port::{GhPort, PullRequest, PullRequestOutcome, SettledPullRequest};

/// More open pull requests than anyone scrolls through in a branch picker.
const LIMIT: &str = "100";

/// The same window over every state rather than only the open ones, so it has to be wider.
/// A branch whose pull request fell outside it is simply not offered — `gh` may only widen
/// the sweep, so the cost of a window too small is fewer marks, never a wrong one.
const SETTLED_LIMIT: &str = "300";

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
            .arg("-R")
            .arg(repo_root)
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

    fn settled_pull_requests(&self, repo_root: &str) -> Option<Vec<SettledPullRequest>> {
        let output = Command::new("gh")
            .arg("-R")
            .arg(repo_root)
            .args([
                "pr",
                "list",
                "--state",
                "all",
                "--limit",
                SETTLED_LIMIT,
                "--json",
                "number,headRefName,state",
            ])
            .current_dir(repo_root)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        // Output this cannot read is not "nothing is merged" either. It means `gh` is
        // answering in a shape this does not know, which is the same not-knowing as `gh`
        // being absent.
        let parsed: Vec<GhSettled> = serde_json::from_slice(&output.stdout).ok()?;
        Some(
            parsed
                .into_iter()
                .filter_map(|pr| {
                    Some(SettledPullRequest {
                        number: pr.number,
                        head_ref: pr.head_ref_name,
                        outcome: match pr.state {
                            GhState::Merged => PullRequestOutcome::Merged,
                            GhState::Closed => PullRequestOutcome::Closed,
                            // Open, or a state a newer `gh` introduces. Neither widens a
                            // sweep, and dropping it is not the same as failing the call.
                            GhState::Other => return None,
                        },
                    })
                })
                .collect(),
        )
    }
}

/// How long a caller should wait for `gh` before giving up on the annotation.
pub const GH_BUDGET: Duration = Duration::from_secs(5);
