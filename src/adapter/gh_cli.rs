//! `GhPort` backed by the `gh` command line.
//!
//! This layer is decoration: it annotates branches with their pull request. Every failure
//! path — `gh` not installed, not authenticated, no network, a repository GitHub has never
//! heard of — degrades to "no pull requests" rather than an error, because the picker has
//! to keep working offline.

use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use crate::port::{GhPort, PullRequest};

/// More open pull requests than anyone scrolls through in a branch picker.
const LIMIT: &str = "100";

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
}

/// How long a caller should wait for `gh` before giving up on the annotation.
pub const GH_BUDGET: Duration = Duration::from_secs(5);
