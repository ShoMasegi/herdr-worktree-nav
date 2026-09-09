//! `GitPort` backed by the `git` command line.

use std::num::NonZeroU32;
use std::process::{Command, Output, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::port::{GitPort, GitRef, RefKind, RefWalk, RepoIdentity, Slug, Track};

/// git's catch-all exit code for a fatal error. It says almost nothing on its own: a path
/// that is not a repository and a fetch that could not reach the remote both exit 128, so
/// the message has to be read to tell them apart.
const GIT_FATAL: i32 = 128;

/// What git says when the path itself is the problem. That is an ordinary answer here — a
/// pane simply is not in a repository — rather than a failure worth reporting.
///
/// git's own English, which is what [`GIT_LOCALE`] is for: the first of these is translated
/// and the second, measured against 2.55.0, is not.
const NOT_A_REPOSITORY: [&str; 2] = ["not a git repository", "cannot change to"];

pub struct GitCli;

/// What git printed when it exited 0.
///
/// Both streams, because a clean exit is not always a whole answer: `for-each-ref` drops a
/// ref it cannot read, says so on stderr, and exits 0 — see `local_refs`. Every other call
/// here reads a clean exit's `stdout` and ignores its `stderr`, which is what they always
/// did; a non-zero exit's `stderr` is what `run` turns into the error, as it always did.
struct Said {
    stdout: String,
    stderr: String,
}

/// The locale every git this plugin starts runs under.
///
/// Two things here are decided by reading git's own words — whether the path is a repository
/// at all ([`NOT_A_REPOSITORY`]) and whether a ref was dropped from a walk ([`dropped_refs`])
/// — and both are English literals, while git ships translations of both messages and picks
/// one from the environment herdr launched the plugin in. Measured against git 2.55.0:
///
/// ```text
/// LC_ALL=C            warning: ignoring broken ref refs/heads/broken
/// LC_ALL=de_DE.UTF-8  Warnung: Ignoriere fehlerhafte Referenz refs/heads/broken
/// LC_ALL=C            fatal: not a git repository (or any of the parent directories): .git
/// LC_ALL=de_DE.UTF-8  Schwerwiegend: Kein Git-Repository (oder irgendeines der …): .git
/// ```
///
/// Under the second of each, a dropped ref goes unnoticed — the walk still exits 0 and the
/// refs it did list still build the markers, so the checkout on the dropped ref carries none
/// and reads exactly like a checkout with nothing to report, with nothing anywhere saying
/// why. That is the silence issue #21 is about. And a pane that is simply not in a
/// repository is read as git refusing.
///
/// `LC_ALL` rather than `LC_MESSAGES` because it outranks the other `LC_*` variables and
/// `LANG`. `LANGUAGE` outranks even `LC_ALL` — `LANGUAGE=fr LC_ALL=de_DE.UTF-8` prints French
/// — but is ignored when the locale is `C`, which is why pinning `LC_ALL` alone is enough:
/// measured with `LANGUAGE=de LC_ALL=de_DE.UTF-8` in the parent and `LC_ALL=C` on the child,
/// which printed git's English.
///
/// The cost is that git's words reach the prompt line and `dump` in English rather than in
/// the reader's language. Everything else the plugin writes is English too, and a sentence
/// this side cannot read is a sentence it cannot act on.
const GIT_LOCALE: (&str, &str) = ("LC_ALL", "C");

impl GitCli {
    /// The command every call here runs, before its arguments: git, in `dir`, with no stdin
    /// and a pinned locale.
    fn command(dir: &str) -> Command {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(dir)
            .stdin(Stdio::null())
            .env(GIT_LOCALE.0, GIT_LOCALE.1);
        command
    }

    /// Run git in `dir`. Returns `None` when git said the path is not a repository; any
    /// other non-zero exit is an error carrying git's words.
    fn run(dir: &str, args: &[&str]) -> Result<Option<Said>> {
        let output: Output = Self::command(dir)
            .args(args)
            .output()
            .map_err(|error| anyhow!("{}", could_not_run(args, &error)))?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            return Ok(Some(Said {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: stderr.into_owned(),
            }));
        }
        // Matching the exit code alone would turn every fatal error into "not a git
        // repository" — a diagnosis that sends the reader looking in entirely the wrong
        // place when what actually happened was that a fetch could not reach the remote.
        if output.status.code() == Some(GIT_FATAL)
            && NOT_A_REPOSITORY.iter().any(|said| stderr.contains(said))
        {
            return Ok(None);
        }
        bail!("{}", refusal(args, &stderr));
    }

    /// Run git where a "not a repository" answer would itself be a bug.
    fn run_in_repo(dir: &str, args: &[&str]) -> Result<Said> {
        Self::run(dir, args)?
            .with_context(|| format!("{dir} is not a git repository, but was expected to be"))
    }
}

/// What a refusal reads as: git's words first, the call after them.
///
/// The other way round put the call first, and for `local_refs` the call carries a
/// `--format=` string 139 columns long: git's words began 191 columns in, past the right
/// edge of every prompt line the picker draws, so the sentence that exists to show them
/// showed the plugin's own argv instead. What a reader needs is what git said; which call
/// it was is the part that can be cut.
fn refusal(args: &[&str], stderr: &str) -> String {
    let said = stderr.trim();
    match said.is_empty() {
        true => format!("git said nothing (`git {}`)", args.join(" ")),
        false => format!("{said} (`git {}`)", args.join(" ")),
    }
}

/// What git said while leaving a ref out of a walk, or nothing.
///
/// The two things `for-each-ref` prints when it drops a ref, measured against git 2.55.0: a
/// loose ref whose contents are not an object id gives `warning: ignoring broken ref
/// refs/heads/x`, and a file under `refs/` whose name is not a legal refname gives
/// `warning: ignoring ref with broken name refs/heads/x`. Both exit 0 and list every other
/// ref.
///
/// In git's English, which git translates and [`GIT_LOCALE`] is what keeps it in.
///
/// Two prefixes rather than "stderr said something", which is what this was and what made a
/// healthy repository read as an unreadable one: `GIT_TRACE`, `GIT_TRACE_PERFORMANCE` and
/// `GIT_TRACE2` are read from the environment herdr launched the plugin in, and each writes
/// a timestamped line per call to stderr while listing every ref correctly. So does an
/// `error: commit-graph file is too small`, which is about the graph and not the refs.
///
/// Only the matching lines are kept, and all of them: git says it once per ref, and a reader
/// told about one who fixes it and is then told about the next has been given half of what
/// git already knew. With a trace variable set as well, the sentence should be git's warning
/// rather than the trace line it happened to follow.
///
/// Not passed through [`refusal`], which is the one place these words are not followed by the
/// call that produced them. It is always this call, so naming it tells a reader nothing they
/// could act on, and it is not small: `refusal` appends 185 columns, 139 of them the
/// `--format=` string.
///
/// Not for reach, though. The call goes on after both refnames and the prompt line cuts from
/// the right, so leaving it off moves the width at which a second refname arrives by exactly
/// one column — the one the ellipsis takes. `src/ui/render.rs` draws both and asserts both
/// widths. What it buys is every width below that one, where the line spends itself on git's
/// words rather than on an argv that is the same on every call; `dump` writes the whole
/// sentence at any width either way.
///
/// A ref git drops in silence — an unreadable directory under `refs/heads`, a dangling
/// symref — is not here and cannot be: the walk exits 0 with nothing said. That is issue
/// #32, and it is what is left of #21 once this branch has closed the rest of it.
fn dropped_refs(stderr: &str) -> Option<String> {
    let dropped: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("warning: ignoring broken ref ")
                || line.starts_with("warning: ignoring ref with broken name ")
        })
        .collect();
    match dropped.is_empty() {
        true => None,
        false => Some(dropped.join(" ")),
    }
}

/// What a git that could not be started reads as: the same shape as a refusal, with the
/// OS's words where git's would be. This is the case the usage page names — a `git` that
/// is not on the path herdr launched the plugin with.
///
/// It reaches the prompt line the way a refusal does for every call made about a repository
/// the tree already has, so it has to read the same way round. Ahead of that it reaches
/// nobody: `app::collect::identify_one` reads a git it could not run as "this pane is not in
/// a repository", so with no git at all the picker draws every pane ungrouped and says
/// nothing about why.
fn could_not_run(args: &[&str], error: &std::io::Error) -> String {
    refusal(args, &format!("git could not be run: {error}"))
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
        // know — including one too large for `u32`. A zero does not: a zero side is a side
        // that is level, so `[ahead 0, behind 2]` is `Behind(2)` and `[ahead 0]` is no
        // marker at all. `NonZeroU32` says that instead of a guard further down. (Real git
        // omits a level side rather than printing a zero, so this is about being right on
        // input rather than about anything `for-each-ref` produces today.)
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
fn github_slug_from_url(url: &str) -> Option<Slug> {
    let url = url.trim();
    let after_host = url
        .split_once("github.com")
        .map(|(_, rest)| rest.trim_start_matches([':', '/']))?;
    let path = after_host.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    Slug::owner_repo(owner, repo)
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
        let mut lines = paths.stdout.lines();
        let (Some(repo_key), Some(checkout_path)) = (lines.next(), lines.next()) else {
            return Ok(None);
        };

        // "HEAD" is what git prints for a detached checkout, which is not a branch name.
        let branch = GitCli::run(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?
            .map(|said| said.stdout.trim().to_string())
            .filter(|s| !s.is_empty() && s != "HEAD");

        Ok(Some(RepoIdentity {
            repo_key: repo_key.trim().to_string(),
            checkout_path: checkout_path.trim().to_string(),
            branch,
        }))
    }

    fn github_slug(&self, repo_root: &str) -> Result<Option<Slug>> {
        // A repository with no `origin` is normal, so a failure here is not an error.
        let Ok(Some(url)) = GitCli::run(repo_root, &["remote", "get-url", "origin"]) else {
            return Ok(None);
        };
        Ok(github_slug_from_url(&url.stdout))
    }

    fn local_refs(&self, repo_root: &str) -> Result<RefWalk> {
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
        let args = [
            "for-each-ref",
            "--format=%(refname)%09%(committerdate:unix)%09%(upstream:short)%09%(upstream:track)%09%(push:track)%09%(worktreepath)%09%(contents:subject)",
            "refs/heads",
            "refs/remotes",
        ];
        let said = GitCli::run_in_repo(repo_root, &args)?;
        let mut refs = Vec::new();
        for line in said.stdout.lines() {
            let mut parts = line.splitn(7, '\t');
            let (Some(refname), Some(date)) = (parts.next(), parts.next()) else {
                continue;
            };
            // Empty for a branch with no upstream configured, and for every remote ref.
            let upstream_name = parts
                .next()
                .map(str::to_string)
                .filter(|name| !name.is_empty());
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
            let upstream_track = parts.next().unwrap_or_default();
            let push = parts.next().unwrap_or_default();
            let track = parse_track(upstream_track)
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
                upstream: upstream_name,
                track,
                worktree_path,
            });
        }
        // A clean exit is not a whole answer here: the refs above are every ref git could
        // read, and a ref it could not is missing from them. That is a checkout with no
        // marker, which is what a branch with nothing to report looks like, so the walk
        // carries git's words for whoever cannot afford the ambiguity — see [`RefWalk`].
        Ok(RefWalk {
            refs,
            dropped: dropped_refs(&said.stderr),
        })
    }

    fn remote_heads(&self, repo_root: &str) -> Result<Vec<String>> {
        let out = GitCli::run_in_repo(repo_root, &["ls-remote", "--heads", "origin"])?;
        Ok(out
            .stdout
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
        Ok(!status.stdout.trim().is_empty())
    }

    fn head_ref(&self, repo_root: &str) -> Result<String> {
        let head = GitCli::run_in_repo(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let head = head.stdout.trim();
        if head.is_empty() || head == "HEAD" {
            // Detached, or an unborn branch in a fresh repository.
            return Ok(GitCli::run_in_repo(repo_root, &["rev-parse", "HEAD"])?
                .stdout
                .trim()
                .to_string());
        }
        Ok(head.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        could_not_run, dropped_refs, github_slug_from_url, parse_track, refusal, GitCli, Slug,
        GIT_LOCALE,
    };
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
        // Syntactically a count, still not one this can hold.
        assert_eq!(parse_track("[ahead 4294967296, behind 2]"), None);
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
            "https://github.com/ShoMasegi/herdr-worktree-nav/",
        ] {
            assert_eq!(
                github_slug_from_url(url).as_ref().map(Slug::as_str),
                Some("ShoMasegi/herdr-worktree-nav"),
                "failed for {url}"
            );
        }
    }

    #[test]
    fn a_refusal_reads_gits_words_first_and_names_the_call_after_them() {
        // The order is the point — see `refusal` — and the empty case has to say that git
        // said nothing, since a sentence that is only a call in parentheses reads as a
        // sentence with its first half missing.
        assert_eq!(
            refusal(&["fetch", "origin"], "fatal: could not read from remote\n"),
            "fatal: could not read from remote (`git fetch origin`)"
        );
        assert_eq!(
            refusal(&["fetch", "origin"], "  \n"),
            "git said nothing (`git fetch origin`)"
        );
    }

    #[test]
    fn the_command_every_call_is_built_from_asks_for_the_locale() {
        // One third of the claim, and only that third: this sees the command the helper
        // hands back, so it says nothing about whether a call uses the helper. Two things
        // here are decided by reading git's English — whether the path is a repository, and
        // whether a ref was dropped — and git translates both, so the other two thirds are
        // `scripts/check-invariants.sh`, which counts the places a git is started, and
        // `tests/git_locale.rs`, which watches a real git obey.
        let command = GitCli::command("/src/app");
        let locale: Vec<_> = command
            .get_envs()
            .filter(|(key, _)| *key == GIT_LOCALE.0)
            .map(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()))
            .collect();
        assert_eq!(locale, vec![Some("C".to_string())]);
    }

    #[test]
    fn only_the_words_git_drops_a_ref_with_are_read_as_a_dropped_ref() {
        // The check this replaced was "stderr said something", and stderr is where git puts
        // everything that is not an answer. A trace variable in the environment herdr
        // launched the plugin in writes a line per call there, and a repository where
        // nothing at all is wrong then lost every marker it had.
        assert_eq!(
            dropped_refs("warning: ignoring broken ref refs/heads/wip\n").as_deref(),
            Some("warning: ignoring broken ref refs/heads/wip")
        );
        assert_eq!(
            dropped_refs("warning: ignoring ref with broken name refs/heads/a..b\n").as_deref(),
            Some("warning: ignoring ref with broken name refs/heads/a..b")
        );
        for said in [
            "",
            "11:35:24.293334 git.c:502  trace: built-in: git for-each-ref\n",
            "error: commit-graph file is too small\n",
            "warning: ignoring something else entirely\n",
        ] {
            assert_eq!(dropped_refs(said), None, "not a dropped ref: {said:?}");
        }

        // Both at once: what the reader gets is git's warning, not the line it followed.
        assert_eq!(
            dropped_refs(
                "11:35:24.293334 git.c:502  trace: built-in: git for-each-ref\n\
                 warning: ignoring broken ref refs/heads/wip\n"
            )
            .as_deref(),
            Some("warning: ignoring broken ref refs/heads/wip")
        );

        // Two broken refs are two warnings — measured, one per ref — and the prompt line is
        // one line, so they are joined with a space rather than run together. All of them,
        // not the first: a reader told about one broken ref who fixes it and finds another
        // has been told half of what git said.
        assert_eq!(
            dropped_refs(
                "warning: ignoring broken ref refs/heads/chore/deps\n\
                 warning: ignoring broken ref refs/heads/feat/login\n"
            )
            .as_deref(),
            Some(
                "warning: ignoring broken ref refs/heads/chore/deps \
                 warning: ignoring broken ref refs/heads/feat/login"
            )
        );

        // Whitespace around a line is defence rather than something git does: `lines()`
        // strips a `\r\n` and keeps only a lone `\r`, and a warning padded either way is
        // still a warning.
        assert_eq!(
            dropped_refs("  warning: ignoring broken ref refs/heads/wip \r\n").as_deref(),
            Some("warning: ignoring broken ref refs/heads/wip")
        );
        // And the lone `\r`, which is the one `lines()` leaves for the trim to take.
        assert_eq!(
            dropped_refs("warning: ignoring broken ref refs/heads/wip\r").as_deref(),
            Some("warning: ignoring broken ref refs/heads/wip")
        );
    }

    #[test]
    fn a_git_that_could_not_be_started_reads_the_same_way_round_as_a_refusal() {
        // The spawn failure used to be the one message built the other way round — the
        // call, then the OS — and it is the failure the usage page names.
        let words = could_not_run(
            &["fetch", "origin"],
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert!(
            words.starts_with("git could not be run: "),
            "the words first: {words}"
        );
        assert!(
            words.ends_with(" (`git fetch origin`)"),
            "the call after them: {words}"
        );
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
