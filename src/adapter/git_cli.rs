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

/// What `remote get-url` says when the remote is not configured: exit 2, with
/// `error: No such remote 'origin'` on stderr, measured against git 2.55.0. The one answer
/// `github_slug` gets that is ordinary rather than a failure — a repository with no `origin`
/// is a repository with no `origin` — and both halves are matched, for the reason
/// [`NOT_A_REPOSITORY`] is not matched by its exit code alone.
///
/// git's own English, translated (`Fehler: Remote-Repository 'origin' nicht gefunden`), which
/// [`GIT_LOCALE`] keeps in place.
const NO_SUCH_REMOTE: (i32, &str) = (2, "No such remote");

pub struct GitCli;

/// What git printed when it exited 0.
///
/// Both streams, because a clean exit is not always a whole answer. `for-each-ref` drops a
/// ref it cannot read, says so on stderr, and exits 0 — see `local_refs` — and `status`
/// leaves out everything under a directory it cannot open the same way — see `is_dirty`.
/// Those two read both streams; the rest read a clean exit's `stdout` alone, and a non-zero
/// exit's `stderr` is what `run` turns into the error.
struct Said {
    stdout: String,
    stderr: String,
}

/// The locale every git this plugin starts runs under.
///
/// Four things here are decided by reading git's own words — whether the path is a
/// repository at all ([`NOT_A_REPOSITORY`]), whether a ref was dropped from a walk
/// ([`dropped_refs`]), whether `status` could open every directory ([`unread_paths`]) and
/// whether a repository simply has no `origin` ([`NO_SUCH_REMOTE`]) — and all are English
/// literals, while git ships a translation of each of these — bar `cannot change to`, the
/// exception [`NOT_A_REPOSITORY`] names — and picks one from the environment herdr launched
/// the plugin in.
///
/// Why this variable, why one git, and what it costs a reader who does not read English:
/// `docs/adr/0015-reading-git-in-one-language.md`, which carries the transcript it was
/// measured from.
const GIT_LOCALE: (&str, &str) = ("LC_ALL", "C");

impl GitCli {
    /// The command every call here runs, before its arguments.
    fn command(dir: &str) -> Command {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(dir)
            .stdin(Stdio::null())
            .env(GIT_LOCALE.0, GIT_LOCALE.1);
        command
    }

    /// Start git in `dir` and wait for it. `Err` is a git that could not be started at all.
    ///
    /// What every call reads its answer from, and the one place the locale is pinned for all
    /// of them. `run` reads it for every call that wants "not a repository" recognised —
    /// `identify` to answer `None`, the eight behind `run_in_repo` to turn it into an error
    /// that says so. `github_slug` reads it itself, because the answer it has to pick out is
    /// a different one and "not a repository" is a failure there rather than an answer.
    fn output(dir: &str, args: &[&str]) -> Result<Output> {
        Self::command(dir)
            .args(args)
            .output()
            .map_err(|error| anyhow!("{}", could_not_run(args, &error)))
    }

    /// Run git in `dir`. Returns `None` when git said the path is not a repository; any
    /// other non-zero exit is an error carrying git's words.
    fn run(dir: &str, args: &[&str]) -> Result<Option<Said>> {
        let output = Self::output(dir, args)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            return Ok(Some(Said {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: stderr.into_owned(),
            }));
        }
        // Matching the exit code alone would turn every fatal error into "not a git
        // repository", a diagnosis that sends the reader looking in entirely the wrong place.
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
/// The other way round puts the call first, and `local_refs` carries a `--format=` string
/// long enough on its own to push git's words past the right edge of every prompt line the
/// picker draws — so the sentence that exists to show them shows the plugin's own argv
/// instead.
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
/// Two prefixes rather than "stderr said something", because stderr carries lines from a
/// walk that went perfectly well — `docs/adr/0015-reading-git-in-one-language.md` names
/// which.
///
/// Only the matching lines are kept, and all of them: git says it once per ref, and a reader
/// told about one who fixes it and is then told about the next has been given half of what
/// git already knew.
///
/// Not passed through [`refusal`], which is the one place these words are not followed by the
/// call that produced them. It is always this call, so naming it tells a reader nothing they
/// could act on, and it is not small: most of what `refusal` appends is the `--format=`
/// string.
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

/// What git said while leaving part of a working tree out of `status`, or nothing.
///
/// The one thing `status --porcelain` prints every time it cannot look, measured against git
/// 2.55.0: a directory it cannot open gives `warning: could not open directory 'notes/':
/// Permission denied`, once per directory, exits 0, and lists nothing under it — tracked or
/// untracked. Where a tracked file is under it git prints a second line beside the warning,
/// `deep/inner/u: Permission denied`, with no prefix; the warning is what is matched, since
/// it is the line that is there in every one of these.
///
/// Measured and not matched, because the answer is whole: a tracked file that is itself
/// unreadable is still reported from the index (` M`), and a tracked path that has gone is
/// ` D` — one line per tracked file, never one for the directory — with nothing on stderr
/// either way.
///
/// Measured and not matched for the other reason: a directory `.gitignore` already excludes
/// is not walked, so git has nothing to warn about and the answer is the same as it would
/// have been. That is what keeps a `node_modules` whose permissions got mangled from turning
/// every row into `?`.
///
/// In git's English, which git translates and [`GIT_LOCALE`] is what keeps it in. One prefix
/// rather than "stderr said something", for the reason `dropped_refs` gives: a trace
/// variable writes to stderr on every call. Every matching line is kept, and not passed
/// through [`refusal`], for one of the reasons given there — it is always this call, so
/// naming it tells a reader nothing they could act on. The other two are `for-each-ref`'s
/// alone: these args are short, and these words reach `dump` rather than the prompt line.
fn unread_paths(stderr: &str) -> Option<String> {
    let unread: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("warning: could not open directory "))
        .collect();
    match unread.is_empty() {
        true => None,
        false => Some(unread.join(" ")),
    }
}

/// Whether git's answer to `remote get-url` is the ordinary "there is no such remote" rather
/// than a failure to name the repository at all.
///
/// Both halves, for the reason `run` gives for [`NOT_A_REPOSITORY`]: an exit code alone
/// diagnoses by number, and git's numbers say almost nothing — `remote get-url` exits 2 for
/// this and 128 for everything from an unparseable config to a root that has gone. The words
/// alone would be no better the day another subcommand's error carries them.
fn no_such_remote(code: Option<i32>, stderr: &str) -> bool {
    let (exit, said) = NO_SUCH_REMOTE;
    code == Some(exit) && stderr.contains(said)
}

/// What a git that could not be started reads as: the same shape as a refusal, with the
/// OS's words where git's would be. This is the case the usage page names — a `git` that
/// is not on the path herdr launched the plugin with.
///
/// Ahead of the prompt line it reaches nobody: `app::collect::identify_one` reads a git it
/// could not run as "this pane is not in a repository", so with no git at all the picker
/// draws every pane ungrouped and says nothing about why.
fn could_not_run(args: &[&str], error: &std::io::Error) -> String {
    refusal(args, &format!("git could not be run: {error}"))
}

/// Read a `:track` field — `%(upstream:track)` or `%(push:track)`, which share a grammar:
/// `[gone]`, `[ahead 2]`, `[behind 1]`, `[ahead 2, behind 1]`, or nothing at all for a branch
/// level with what it is being compared against, or with nothing to compare against.
///
/// `None` is that last shape, and [`Track::Unreadable`] is a field git printed that this
/// cannot read — two answers a caller owes different things, which is why neither is the
/// other's `None`. Nothing a git in use prints lands on the second: `for-each-ref` is
/// plumbing, printing these counts as integers in a grammar of its own that no locale
/// touches. Reaching it takes a count past `u32` or a git that changed that grammar, and
/// the second is what this is for — a grammar this side does not know is not a branch that
/// is level with its upstream, however much the empty marker they share suggests it.
///
/// What `[gone]` *means* differs between the two fields, which is why the caller and not this
/// function decides whether to believe it.
fn parse_track(field: &str) -> Option<Track> {
    let field = field.trim();
    if field.is_empty() {
        return None;
    }
    let Some(inside) = field
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return Some(Track::Unreadable);
    };
    if inside == "gone" {
        return Some(Track::Gone);
    }
    let mut ahead = None;
    let mut behind = None;
    for part in inside.split(", ") {
        // A count this cannot read fails the whole field, the same as a word it does not
        // know — including one too large for `u32`. A zero does not: a zero side is a side
        // that is level, so `[ahead 0, behind 2]` is `Behind(2)` and `[ahead 0]` is a field
        // that reports nothing. `NonZeroU32` says that instead of a guard further down.
        // (Real git omits a level side rather than printing a zero, so this is about being
        // right on input rather than about anything `for-each-ref` produces today.)
        let Some((side, count)) = part.split_once(' ') else {
            return Some(Track::Unreadable);
        };
        let Ok(count) = count.parse::<u32>() else {
            return Some(Track::Unreadable);
        };
        match side {
            "ahead" => ahead = NonZeroU32::new(count),
            "behind" => behind = NonZeroU32::new(count),
            _ => return Some(Track::Unreadable),
        }
    }
    match (ahead, behind) {
        (Some(ahead), Some(behind)) => Some(Track::Diverged { ahead, behind }),
        (Some(ahead), None) => Some(Track::Ahead(ahead)),
        (None, Some(behind)) => Some(Track::Behind(behind)),
        (None, None) => None,
    }
}

/// Read a ref's position from what git printed for it: the upstream it tracks, and the two
/// `:track` fields.
///
/// `%(upstream:track)` is the question this plugin is asking. `%(push:track)` answers a
/// different one — where the branch stands against the ref git would push it to — and the
/// two can name different refs: under `push.default = current` the push destination of a
/// local branch tracking `origin/main` is `origin/<the branch's own name>`. A marker drawn
/// off the second and presented as the first is a comparison the user never asked for, with
/// nothing on the row to tell it from the one they did.
///
/// So the push side is read in one case only: git named no upstream, so there was no first
/// question and no answer to misrepresent. The one other shape that reaches here empty
/// handed is an empty field beside an upstream, which is git saying the branch is level with
/// it, and the upstream name is what tells the two apart. A field git printed and this could
/// not read leaves [`parse_track`] as [`Track::Unreadable`] and is neither of them.
///
/// `gone` off the push side is dropped whichever way it got there. Under
/// `push.default = current` or `matching`, the push destination of a branch nobody has
/// pushed yet resolves to a ref that has never existed, and git reports that as `[gone]` —
/// the opposite of what this marker means, on the branches where being wrong matters most:
/// `docs/adr/0011-what-may-be-swept.md` makes `gone` the signal a sweep marks a branch for
/// deletion on, and an unpushed branch is the one kind that exists nowhere else.
fn read_track(upstream: Option<&str>, upstream_track: &str, push_track: &str) -> Option<Track> {
    match (parse_track(upstream_track), upstream) {
        (Some(track), _) => Some(track),
        // git named an upstream and reported nothing about it, which is the answer: the
        // branch is level with it. The push ref is not the ref that answer is about.
        (None, Some(_)) => None,
        (None, None) => match parse_track(push_track) {
            Some(Track::Gone) => None,
            track => track,
        },
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
        let args = ["remote", "get-url", "origin"];
        let output = GitCli::output(repo_root, &args)?;
        if output.status.success() {
            let url = String::from_utf8_lossy(&output.stdout);
            return Ok(github_slug_from_url(&url));
        }
        // A repository with no `origin` is ordinary, and it is the one non-zero exit here
        // that is not a failure. Every other one is git failing to answer — a `.git/config`
        // it cannot parse, a repository root that has gone — and reading them all as "no
        // remote" sends the user whose repository is gone to look at their remotes, which
        // is issue #27. What `repo_root` names is a repository herdr listed, so here "not a
        // repository" is git failing to name it, not an ordinary answer, and it goes up
        // with the rest.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if no_such_remote(output.status.code(), &stderr) {
            return Ok(None);
        }
        bail!("{}", refusal(&args, &stderr));
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
        // checkout path could too, which would mis-split the line, and nothing here can
        // prevent that.
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
            let upstream_track = parts.next().unwrap_or_default();
            let push_track = parts.next().unwrap_or_default();
            let track = read_track(upstream_name.as_deref(), upstream_track, push_track);
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
        // The refs above are every ref git could read, and a ref it could not is missing
        // from them, so the walk carries git's words for whoever cannot afford to read that
        // silence as nothing to report — see [`Said`] and [`RefWalk`].
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

    fn delete_branch(&self, repo_root: &str, branch: &str) -> Result<()> {
        // `-d` and not `-D`, with nothing here that escalates, for the reason
        // `docs/adr/0011-what-may-be-swept.md` gives under "Never `-D`". That git refuses a
        // squash-merged branch is held by
        // `a_branch_git_does_not_call_merged_is_kept_and_git_says_why` in
        // `tests/git_adapter.rs`.
        GitCli::run_in_repo(repo_root, &["branch", "-d", branch])?;
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
        //
        // `status.showUntrackedFiles` is pinned for the same reason `push.default` and the
        // locale are: the answer is only worth something if this side chose the question.
        // Set to `no` anywhere in the chain — repository, global, system — git exits 0 with
        // nothing to say about an ordinary untracked file, and a sweep deletes the checkout
        // holding it. Issue #68.
        let status = GitCli::run_in_repo(
            checkout_path,
            &[
                "-c",
                "status.showUntrackedFiles=all",
                "--no-optional-locks",
                "status",
                "--porcelain",
            ],
        )?;
        // A clean exit is not a whole answer here either. A directory git could not open is
        // reported on stderr and everything under it is simply absent from stdout, so an
        // empty stdout beside that warning is not `clean` — it is the answer git could not
        // give, about the one working tree a sweep would then act on. `Err`, because a
        // `bool` has nowhere to carry the words; `app::dirty` reads it as
        // `WorkingTree::Unreadable`, and `dump` asks again and prints what git says then.
        //
        // Whatever stdout holds, not only when it is empty. A checkout can have visible work
        // *and* a directory git could not open, and `true` would then be a whole answer to
        // the question asked — but it is a whole answer arrived at by accident, from the
        // half git could see. A partial reading is not reported as a complete one because
        // the visible half happened to land on the safe side; the row says `?` rather than
        // `✱`, and both refuse a sweep.
        if let Some(words) = unread_paths(&status.stderr) {
            bail!("{words}");
        }
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
        could_not_run, dropped_refs, github_slug_from_url, no_such_remote, parse_track, read_track,
        refusal, unread_paths, GitCli, Slug, GIT_LOCALE,
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
    fn a_field_that_reports_nothing_is_told_from_one_that_could_not_be_read() {
        // Which of the two rules an input is under is the whole of what the caller reads,
        // and the absence is only ever the first of them.
        //
        // Nothing to report: level with what it was measured against, or nothing to be
        // measured against. Both print empty, and a zero side is the same fact written out.
        for field in ["", "   ", "[ahead 0]", "[behind 0]", "[ahead 0, behind 0]"] {
            assert_eq!(parse_track(field), None, "for {field:?}");
        }
        // Could not be read: git said something, and this side does not know what.
        for field in ["[ahead many]", "[sideways 2]", "gone", "[]"] {
            assert_eq!(parse_track(field), Some(Track::Unreadable), "for {field:?}");
        }
    }

    #[test]
    fn a_count_that_cannot_be_read_fails_the_whole_field() {
        // Not just its own half. Believing the side that parsed would put a marker on the
        // row that is right about one direction and silent about the other, which reads as
        // a branch that is only ahead — a claim nothing in the input supports.
        assert_eq!(
            parse_track("[ahead 2, behind zzz]"),
            Some(Track::Unreadable)
        );
        assert_eq!(
            parse_track("[ahead zzz, behind 1]"),
            Some(Track::Unreadable)
        );
        // Syntactically a count, still not one this can hold.
        assert_eq!(
            parse_track("[ahead 4294967296, behind 2]"),
            Some(Track::Unreadable)
        );
    }

    #[test]
    fn the_push_side_is_read_for_a_branch_git_named_no_upstream_for_and_for_no_other() {
        let behind = Track::Behind(NonZeroU32::new(3).unwrap());
        // The one case the second question belongs to: nothing was asked first, so nothing
        // is being misrepresented by the answer.
        assert_eq!(read_track(None, "", "[behind 3]"), Some(behind));
        // Level with the upstream git named. The empty field is git's answer to the first
        // question, and the push ref is not the ref it answered about.
        assert_eq!(read_track(Some("origin/main"), "", "[behind 3]"), None);
        // A field this could not read, with or without an upstream: the answer went unread,
        // which is not the same as never having been asked. The push side is not read for
        // it either — a question that went unanswered is still the question that was asked.
        assert_eq!(
            read_track(None, "[ahead many]", "[behind 3]"),
            Some(Track::Unreadable)
        );
        assert_eq!(
            read_track(Some("origin/main"), "[ahead many]", "[behind 3]"),
            Some(Track::Unreadable)
        );
        // And a field that was read wins outright, whatever the push side says.
        assert_eq!(
            read_track(Some("origin/main"), "[ahead 2]", "[behind 3]"),
            Some(Track::Ahead(NonZeroU32::new(2).unwrap()))
        );
    }

    #[test]
    fn a_push_destination_that_has_never_existed_is_not_a_branch_whose_upstream_is_gone() {
        assert_eq!(read_track(None, "", "[gone]"), None);
        assert_eq!(
            read_track(Some("origin/main"), "[gone]", ""),
            Some(Track::Gone)
        );
        // A push side this could not read draws no marker either, and says so in the one
        // place it is the answer: the branch git named no upstream for.
        assert_eq!(
            read_track(None, "", "[ahead many]"),
            Some(Track::Unreadable)
        );
    }

    #[test]
    fn a_branch_a_field_went_unread_for_is_not_a_branch_that_is_level() {
        // The two shapes `dump` has to tell apart, from one function: `level` is a claim
        // that the branch is even with the ref git named, and the second input supports no
        // claim at all.
        assert_eq!(read_track(Some("origin/main"), "", ""), None);
        assert_eq!(
            read_track(Some("origin/main"), "[sideways 2]", ""),
            Some(Track::Unreadable)
        );
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
        // The empty case has to say that git said nothing, since a sentence that is only a
        // call in parentheses reads as one with its first half missing.
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
        // hands back, so it says nothing about whether a call uses the helper. The other two
        // thirds are `scripts/check-invariants.sh`, which counts the places a git is
        // started, and `tests/git_locale.rs`, which watches a real git obey.
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
        // "stderr said something" is the tempting check and the wrong one: stderr is where
        // git puts everything that is not an answer. A trace variable in the environment
        // herdr launched the plugin in writes a line per call there, and under that check a
        // repository where nothing at all is wrong loses every marker it has.
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
        // one line, so they are joined with a space rather than run together.
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
        assert_eq!(
            dropped_refs("warning: ignoring broken ref refs/heads/wip\r").as_deref(),
            Some("warning: ignoring broken ref refs/heads/wip")
        );
    }

    #[test]
    fn only_the_words_git_leaves_a_directory_out_with_are_read_as_an_unread_one() {
        // The same rule as for a dropped ref: stderr is where git puts everything that is
        // not an answer, so one prefix and not "stderr said something".
        assert_eq!(
            unread_paths("warning: could not open directory 'notes/': Permission denied\n")
                .as_deref(),
            Some("warning: could not open directory 'notes/': Permission denied")
        );
        for said in [
            "",
            "11:35:24.293334 git.c:502  trace: built-in: git status\n",
            "warning: ignoring broken ref refs/heads/wip\n",
            // The second line git prints beside the warning for a tracked file under the
            // directory. Alone it is not the answer — it never comes alone — and the
            // warning it accompanies is the line that is matched.
            "deep/inner/u: Permission denied\n",
        ] {
            assert_eq!(
                unread_paths(said),
                None,
                "not an unread directory: {said:?}"
            );
        }

        // Both lines git prints for a tracked file under such a directory: the warning is
        // kept and the bare line is not, so what reaches the reader is one shape of sentence.
        assert_eq!(
            unread_paths(
                "deep/inner/u: Permission denied\n\
                 warning: could not open directory 'deep/inner/': Permission denied\n"
            )
            .as_deref(),
            Some("warning: could not open directory 'deep/inner/': Permission denied")
        );

        // Two directories are two warnings, joined with a space for the one line.
        assert_eq!(
            unread_paths(
                "warning: could not open directory 'notes/': Permission denied\n\
                 warning: could not open directory 'deep/inner/': Permission denied\n"
            )
            .as_deref(),
            Some(
                "warning: could not open directory 'notes/': Permission denied \
                 warning: could not open directory 'deep/inner/': Permission denied"
            )
        );
    }

    #[test]
    fn a_repository_with_no_origin_is_told_from_one_git_could_not_name() {
        // Measured against git 2.55.0: `remote get-url origin` exits 2 saying `No such
        // remote` where there is none, and 128 for a config it cannot parse or a root that
        // has gone. Both halves are read, the way `run` reads `NOT_A_REPOSITORY`.
        assert!(no_such_remote(Some(2), "error: No such remote 'origin'\n"));

        for (code, stderr) in [
            // The two fatals this used to swallow, which are what issue #27 is about.
            (Some(128), "fatal: bad config line 9 in file .git/config\n"),
            (
                Some(128),
                "fatal: cannot change to '/wt/gone': No such file or directory\n",
            ),
            // The words on their own, and the number on its own. Neither is the answer.
            (Some(128), "error: No such remote 'origin'\n"),
            (Some(2), "error: something else entirely\n"),
            // Killed by a signal: no code at all.
            (None, "error: No such remote 'origin'\n"),
        ] {
            assert!(
                !no_such_remote(code, stderr),
                "not a missing remote: {code:?} {stderr:?}"
            );
        }
    }

    #[test]
    fn a_git_that_could_not_be_started_reads_the_same_way_round_as_a_refusal() {
        // The spawn failure is the one easiest to build the other way round — the call,
        // then the OS — because the OS's words are not git's.
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
