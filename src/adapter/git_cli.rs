//! `GitPort` backed by the `git` command line.

use std::num::NonZeroU32;
use std::process::{Command, Output, Stdio};

use anyhow::{bail, Context, Result};

use crate::port::{GitPort, GitRef, RefKind, RepoIdentity, Track};

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

/// Read a `:track` field — `%(upstream:track)` or `%(push:track)`, which share a grammar:
/// `[gone]`, `[ahead 2]`, `[behind 1]`, `[ahead 2, behind 1]`, or nothing at all for a branch
/// level with what it is being compared against, or with nothing to compare against.
///
/// What `[gone]` *means* differs between the two, which is why the caller and not this
/// function decides whether to believe it.
///
/// Anything unrecognised is `None` rather than a guess. A marker that is wrong is worse than
/// no marker, because the whole point of these is to answer "which of these is behind"
/// without leaving the picker to check.
fn parse_track(field: &str) -> Option<Track> {
    let inside = field.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inside == "gone" {
        return Some(Track::Gone);
    }
    let mut ahead = None;
    let mut behind = None;
    for part in inside.split(", ") {
        // A count this cannot read fails the whole field, the same as a word it does not
        // know. A zero does not: git prints `[ahead 0]` for nobody, so that side is simply
        // level, and `NonZeroU32` is what says so instead of a guard further down.
        match part.split_once(' ') {
            Some(("ahead", count)) => ahead = NonZeroU32::new(count.parse().ok()?),
            Some(("behind", count)) => behind = NonZeroU32::new(count.parse().ok()?),
            _ => return None,
        }
    }
    match (ahead, behind) {
        (Some(ahead), Some(behind)) => Some(Track::Diverged { ahead, behind }),
        (Some(ahead), None) => Some(Track::Ahead(ahead)),
        (None, Some(behind)) => Some(Track::Behind(behind)),
        (None, None) => None,
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
        //
        // Everything in one format string. Not because the extra fields are free —
        // `:track` costs git an ahead/behind walk per ref — but because they arrive in the
        // one process that was being started anyway. Asking separately would be a
        // `rev-list --count` per branch and a `worktree list` on top.
        //
        // The subject goes last because it is the field most likely to contain a tab. A
        // checkout path could too, which would mis-split the line; nothing here can prevent
        // that, and a path with a tab in it would be the least of that user's problems.
        let out = GitCli::run_in_repo(
            repo_root,
            &[
                "for-each-ref",
                "--format=%(refname)%09%(committerdate:unix)%09%(upstream:track)%09%(push:track)%09%(worktreepath)%09%(contents:subject)",
                "refs/heads",
                "refs/remotes",
            ],
        )?;

        let mut refs = Vec::new();
        for line in out.lines() {
            let mut parts = line.splitn(6, '\t');
            let (Some(refname), Some(date)) = (parts.next(), parts.next()) else {
                continue;
            };
            // A branch with no upstream configured but a push destination still has
            // somewhere to be ahead of, and `push:track` is where git says so.
            //
            // It may not say `gone`, though. Under `push.default = current` or `matching`,
            // the push destination of a branch nobody has pushed yet resolves to a ref that
            // has never existed, and git reports that as `[gone]` — the opposite of what
            // this marker means, on the branches where being wrong matters most:
            // `docs/adr/0011-what-may-be-swept.md` makes `gone` the signal a sweep marks a
            // branch for deletion on, and an unpushed branch is the one kind that exists
            // nowhere else.
            let upstream = parts.next().unwrap_or_default();
            let push = parts.next().unwrap_or_default();
            let track = parse_track(upstream)
                .or_else(|| parse_track(push).filter(|track| *track != Track::Gone));
            let worktree_path = parts
                .next()
                .map(str::to_string)
                .filter(|path| !path.is_empty());
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
                track,
                worktree_path,
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

    fn is_dirty(&self, checkout_path: &str) -> Result<bool> {
        // `--no-optional-locks` because of where this runs: on every checkout at once, in
        // the background, in the very working trees the session's agents are committing in.
        // Plain `git status` refreshes the index as a side effect and takes `index.lock` to
        // do it — git's own documentation offers this flag to turn that off — and a picker
        // that only looks at a repository has no business making somebody's `git commit`
        // fail.
        //
        // Untracked files count, because they count to `git worktree remove`: the marker has
        // to mean the same thing there as it does on `Shift-D`, or it is telling the user
        // something they cannot act on.
        let status = GitCli::run_in_repo(
            checkout_path,
            &["--no-optional-locks", "status", "--porcelain"],
        )?;
        Ok(!status.trim().is_empty())
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
    use super::{github_slug_from_url, parse_track};
    use crate::port::Track;
    use std::num::NonZeroU32;

    #[test]
    fn reads_every_shape_git_prints_for_upstream_track() {
        assert_eq!(parse_track("[gone]"), Some(Track::Gone));
        assert_eq!(
            parse_track("[ahead 2]"),
            Some(Track::Ahead(NonZeroU32::new(2).unwrap()))
        );
        assert_eq!(
            parse_track("[behind 1]"),
            Some(Track::Behind(NonZeroU32::new(1).unwrap()))
        );
        assert_eq!(
            parse_track("[ahead 2, behind 1]"),
            Some(Track::Diverged {
                ahead: NonZeroU32::new(2).unwrap(),
                behind: NonZeroU32::new(1).unwrap()
            })
        );
    }

    #[test]
    fn a_branch_with_nothing_to_report_gets_no_marker() {
        // Level with its upstream, and no upstream at all, both print nothing — and both
        // mean there is nothing to draw.
        assert_eq!(parse_track(""), None);
        assert_eq!(parse_track("   "), None);
    }

    #[test]
    fn a_count_that_cannot_be_read_fails_the_whole_field() {
        // Not just its own half. Believing the side that parsed would put a marker on the
        // row that is right about one direction and silent about the other, which reads as
        // a branch that is only ahead — a claim nothing in the input supports.
        assert_eq!(parse_track("[ahead 2, behind zzz]"), None);
        assert_eq!(parse_track("[ahead zzz, behind 1]"), None);
    }

    #[test]
    fn anything_unrecognised_is_no_marker_rather_than_a_guess() {
        // A marker that is wrong is worse than none: these exist so the user does not have
        // to leave the picker to check.
        assert_eq!(parse_track("[ahead many]"), None);
        assert_eq!(parse_track("[sideways 2]"), None);
        assert_eq!(parse_track("gone"), None);
        assert_eq!(parse_track("[ahead 0]"), None);
    }

    #[test]
    fn extracts_the_slug_from_every_github_remote_form() {
        for url in [
            "https://github.com/ShoMasegi/herdr-worktree-nav.git",
            "https://github.com/ShoMasegi/herdr-worktree-nav",
            "git@github.com:ShoMasegi/herdr-worktree-nav.git",
            "ssh://git@github.com/ShoMasegi/herdr-worktree-nav.git",
            "  https://github.com/ShoMasegi/herdr-worktree-nav.git\n",
        ] {
            assert_eq!(
                github_slug_from_url(url).as_deref(),
                Some("ShoMasegi/herdr-worktree-nav"),
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
