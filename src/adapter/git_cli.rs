//! `GitPort` backed by the `git` command line.

use std::process::{Command, Output, Stdio};

use anyhow::{bail, Context, Result};

use crate::port::{GitPort, GitRef, RefKind, RepoIdentity};

/// git's catch-all exit code for a fatal error. It says almost nothing on its own: a path
/// that is not a repository and a fetch that could not reach the remote both exit 128, so
/// the message has to be read to tell them apart.
const GIT_FATAL: i32 = 128;

/// What git says when the path itself is the problem. That is an ordinary answer here — a
/// pane simply is not in a repository — rather than a failure worth reporting.
const NOT_A_REPOSITORY: [&str; 2] = ["not a git repository", "cannot change to"];

pub struct GitCli;

impl GitCli {
    /// Run git in `dir` and capture stdout. Returns `None` when git said the path is not a
    /// repository; any other non-zero exit is an error.
    fn run(dir: &str, args: &[&str]) -> Result<Option<String>> {
        let output: Output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("running `git {}`", args.join(" ")))?;

        if output.status.success() {
            return Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Matching the exit code alone would turn every fatal error into "not a git
        // repository" — a diagnosis that sends the reader looking in entirely the wrong
        // place when what actually happened was that a fetch could not reach the remote.
        if output.status.code() == Some(GIT_FATAL)
            && NOT_A_REPOSITORY.iter().any(|said| stderr.contains(said))
        {
            return Ok(None);
        }
        bail!("`git {}` failed: {}", args.join(" "), stderr.trim());
    }

    /// Run git where a "not a repository" answer would itself be a bug.
    fn run_in_repo(dir: &str, args: &[&str]) -> Result<String> {
        Self::run(dir, args)?
            .with_context(|| format!("{dir} is not a git repository, but was expected to be"))
    }
}

/// Extract `owner/repo` from any GitHub remote URL form:
/// `https://github.com/o/r.git`, `git@github.com:o/r.git`, `ssh://git@github.com/o/r`.
fn github_slug_from_url(url: &str) -> Option<String> {
    let url = url.trim();
    let after_host = url
        .split_once("github.com")
        .map(|(_, rest)| rest.trim_start_matches([':', '/']))?;
    let path = after_host.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

impl GitPort for GitCli {
    fn identify(&self, cwd: &str) -> Result<Option<RepoIdentity>> {
        // --path-format=absolute matters: --git-common-dir prints a bare ".git" when the
        // query runs from the repository root, which would not be a usable identity.
        let Some(paths) = GitCli::run(
            cwd,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
                "--show-toplevel",
            ],
        )?
        else {
            return Ok(None);
        };
        let mut lines = paths.lines();
        let (Some(repo_key), Some(checkout_path)) = (lines.next(), lines.next()) else {
            return Ok(None);
        };

        // "HEAD" is what git prints for a detached checkout, which is not a branch name.
        let branch = GitCli::run(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "HEAD");

        Ok(Some(RepoIdentity {
            repo_key: repo_key.trim().to_string(),
            checkout_path: checkout_path.trim().to_string(),
            branch,
        }))
    }

    fn github_slug(&self, repo_root: &str) -> Result<Option<String>> {
        // A repository with no `origin` is normal, so a failure here is not an error.
        let Ok(Some(url)) = GitCli::run(repo_root, &["remote", "get-url", "origin"]) else {
            return Ok(None);
        };
        Ok(github_slug_from_url(&url))
    }

    fn local_refs(&self, repo_root: &str) -> Result<Vec<GitRef>> {
        // The full refname, not `refname:short`: a local branch called `feat/login` and a
        // remote ref printed as `origin/main` are indistinguishable once shortened.
        let out = GitCli::run_in_repo(
            repo_root,
            &[
                "for-each-ref",
                "--format=%(refname)%09%(committerdate:unix)%09%(contents:subject)",
                "refs/heads",
                "refs/remotes",
            ],
        )?;

        let mut refs = Vec::new();
        for line in out.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(refname), Some(date)) = (parts.next(), parts.next()) else {
                continue;
            };
            let subject = parts.next().map(str::to_string).filter(|s| !s.is_empty());

            let (name, kind) = if let Some(branch) = refname.strip_prefix("refs/heads/") {
                (branch, RefKind::Local)
            } else if let Some(rest) = refname.strip_prefix("refs/remotes/") {
                // `refs/remotes/<remote>/<branch>` — drop the remote so the name is the
                // branch as it exists upstream, which is what a worktree is cut from.
                let Some((_remote, branch)) = rest.split_once('/') else {
                    continue;
                };
                // `origin/HEAD` is a symbolic alias for the default branch, not a branch.
                if branch == "HEAD" {
                    continue;
                }
                (branch, RefKind::Remote)
            } else {
                continue;
            };

            if name.is_empty() {
                continue;
            }
            refs.push(GitRef {
                name: name.to_string(),
                kind,
                committed_at: date.parse().ok(),
                subject,
            });
        }
        Ok(refs)
    }

    fn remote_heads(&self, repo_root: &str) -> Result<Vec<String>> {
        let out = GitCli::run_in_repo(repo_root, &["ls-remote", "--heads", "origin"])?;
        Ok(out
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter_map(|(_sha, r)| r.strip_prefix("refs/heads/"))
            .map(str::to_string)
            .collect())
    }

    fn fetch_branch(&self, repo_root: &str, branch: &str) -> Result<()> {
        // Fetch straight into the remote-tracking ref rather than FETCH_HEAD, so the branch
        // afterwards has a real `origin/<branch>` to be based on. The leading `+` allows a
        // non-fast-forward update, and writing to refs/remotes never touches a checked-out
        // branch, so this is safe whatever the repository is currently on.
        let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
        GitCli::run_in_repo(repo_root, &["fetch", "origin", &refspec])?;
        Ok(())
    }

    fn fetch_all(&self, repo_root: &str) -> Result<()> {
        // `--prune` is what makes this a refresh rather than an accumulation: without it a
        // branch deleted on the remote stays in `refs/remotes` and so stays in the list,
        // for ever. It deletes only remote-tracking refs, which are a cache of the remote
        // and are rebuilt by the next fetch; no local branch and no working tree is touched.
        GitCli::run_in_repo(repo_root, &["fetch", "origin", "--prune"])?;
        Ok(())
    }

    fn remove_worktree(&self, repo_root: &str, checkout_path: &str) -> Result<()> {
        // No `--force`. git refuses a checkout with uncommitted work or untracked files,
        // and that refusal is the point: the picker has no business deciding that work
        // nobody has committed is disposable.
        GitCli::run_in_repo(repo_root, &["worktree", "remove", checkout_path])?;
        Ok(())
    }

    fn head_ref(&self, repo_root: &str) -> Result<String> {
        let head = GitCli::run_in_repo(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let head = head.trim();
        if head.is_empty() || head == "HEAD" {
            // Detached, or an unborn branch in a fresh repository.
            return Ok(GitCli::run_in_repo(repo_root, &["rev-parse", "HEAD"])?
                .trim()
                .to_string());
        }
        Ok(head.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::github_slug_from_url;

    #[test]
    fn extracts_the_slug_from_every_github_remote_form() {
        for url in [
            "https://github.com/ShoMasegi/herdr-gh-nav.git",
            "https://github.com/ShoMasegi/herdr-gh-nav",
            "git@github.com:ShoMasegi/herdr-gh-nav.git",
            "ssh://git@github.com/ShoMasegi/herdr-gh-nav.git",
            "  https://github.com/ShoMasegi/herdr-gh-nav.git\n",
        ] {
            assert_eq!(
                github_slug_from_url(url).as_deref(),
                Some("ShoMasegi/herdr-gh-nav"),
                "failed for {url}"
            );
        }
    }

    #[test]
    fn rejects_remotes_that_are_not_github() {
        for url in [
            "git@gitlab.com:owner/repo.git",
            "https://example.com/owner/repo.git",
            "/srv/git/bare-repo.git",
            "",
        ] {
            assert_eq!(github_slug_from_url(url), None, "failed for {url}");
        }
    }
}
