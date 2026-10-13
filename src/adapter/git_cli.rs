//! `GitPort` backed by the `git` command line.

use std::num::NonZeroU32;
use std::process::{Command, Output, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::port::{GitPort, GitRef, RefKind, RefWalk, RepoIdentity, Slug, Track};

/// git's catch-all exit code for a fatal error. It says almost nothing on its own: a path
/// that is not a repository and a fetch that could not reach the remote both exit 128, so
/// the message has to be read to tell them apart.
const GIT_FATAL: i32 = 128;

/// What git says when the path is in no repository. That is an ordinary answer here — a
/// pane simply is not in a repository — rather than a failure worth reporting.
///
/// git's own English, translated, which is what [`GIT_LOCALE`] is for.
const NOT_A_REPOSITORY: &str = "not a git repository";

/// What git says when the path it was pointed at is not there: no directory, so no
/// repository, which is the same ordinary answer as [`NOT_A_REPOSITORY`]. A pane left in a
/// checkout another pane removed.
///
/// Both halves, because the first is what git says of every directory it could not enter
/// and only the second says why: `Permission denied`, or a network mount that has gone
/// stale, is a directory git never looked in, and reading it as "no repository" would put
/// the pane among those in none with nothing to say why. Both are English only under
/// [`GIT_LOCALE`]: git translates `fatal: ` and, measured against 2.55.0, leaves
/// `cannot change to` in English here; the second half is the C library's.
const CANNOT_CHANGE_TO: (&str, &str) = ("fatal: cannot change to ", ": No such file or directory");

/// What git says when the path is in a repository but in no work tree: a bare repository, or
/// a checkout's `.git` directory and the directories git keeps in it, `refs` and `objects`
/// among them. Not a submodule's directory under `.git/modules`, which names its work tree,
/// so git answers there with the submodule's checkout. To `identify` that is the same answer
/// as [`NOT_A_REPOSITORY`] — there is no work tree there for git to place a pane in — and
/// only `identify` reads it so; to a call that needs a work tree, it is the error.
///
/// Translated, like [`NOT_A_REPOSITORY`], and measured against 2.55.0.
const NOT_A_WORK_TREE: &str = "this operation must be run in a work tree";

/// What `remote get-url` says when the remote is not configured: exit 2, with
/// `error: No such remote 'origin'` on stderr, measured against git 2.55.0. The one answer
/// `github_slug` gets that is ordinary rather than a failure — a repository with no `origin`
/// is a repository with no `origin` — and both halves are matched, for the reason
/// [`NOT_A_REPOSITORY`] is not matched by its exit code alone.
///
/// git's own English, translated (`Fehler: Remote-Repository 'origin' nicht gefunden`), which
/// [`GIT_LOCALE`] keeps in place.
const NO_SUCH_REMOTE: (i32, &str) = (2, "No such remote");

/// What `log` says of a branch that has no commit yet: exit 128, with `fatal: your current
/// branch 'main' does not have any commits yet` on stderr, measured against git 2.55.0. A
/// branch whose ref git cannot read gets `fatal: your current branch appears to be broken`
/// under the same exit, and is the answer that has to stay a refusal — so the words are
/// matched with the exit rather than the exit alone, as [`NO_SUCH_REMOTE`]'s are.
///
/// git's own English, translated (`Schwerwiegend: Ihr aktueller Branch 'main' hat noch keine
/// Commits.`), which [`GIT_LOCALE`] keeps in place.
const NO_COMMITS_YET: (i32, &str) = (GIT_FATAL, "does not have any commits yet");

pub struct GitCli;

/// What git printed when it exited 0.
///
/// Both streams, because a clean exit is not always a whole answer. `for-each-ref` drops a
/// ref it cannot read, says so on stderr, and exits 0 — see `local_refs` — and `status`
/// says the same way that it left part of the tree out, or read it by rules it could not
/// see — see `is_dirty`.
/// Those two read both streams; the rest read a clean exit's `stdout` alone, and a non-zero
/// exit's `stderr` is what `run` turns into the error.
struct Said {
    stdout: String,
    stderr: String,
}

/// The locale every git this plugin starts runs under.
///
/// Five things here are decided by reading git's own words — whether the path is a
/// repository at all ([`NOT_A_REPOSITORY`] and [`CANNOT_CHANGE_TO`], and [`NOT_A_WORK_TREE`]
/// for `identify`), whether a ref was dropped from a walk ([`dropped_refs`]), whether
/// `status` could look at all of the working tree ([`unread_paths`]), whether a repository
/// simply has no `origin` ([`NO_SUCH_REMOTE`]) and whether a branch has no commit yet
/// ([`NO_COMMITS_YET`]) — and all are English literals,
/// while git translates each of these — bar the words `cannot change to`, which it leaves in
/// English where [`CANNOT_CHANGE_TO`] reads them, and the words after the last colon of
/// [`CANNOT_CHANGE_TO`]'s and [`unread_paths`]' lines, which are the C library's — and picks
/// one from the environment herdr launched the plugin in.
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
    /// What every call but one reads its answer from. `run_answering` reads it for every
    /// call that wants "not here" recognised — `identify` to answer `None`, and through
    /// `run` the eight behind `run_in_repo`, to turn "not a repository" into an error that
    /// says so. `github_slug` reads it itself, because the answer it has to pick out is a
    /// different one and "not a repository" is a failure there rather than an answer. The
    /// one call that does not come through here is `identify`'s read of the branch, which
    /// starts from `command` and so runs in the same locale.
    fn output(dir: &str, args: &[&str]) -> Result<Output> {
        Self::command(dir)
            .args(args)
            .output()
            .map_err(|error| anyhow!("{}", could_not_run(args, &error)))
    }

    /// Run git in `dir`. Returns `None` when git said the path is not a repository; any
    /// other non-zero exit is an error carrying git's words.
    fn run(dir: &str, args: &[&str]) -> Result<Option<Said>> {
        Self::run_answering(dir, args, no_repository_here)
    }

    /// `run`, with what reads as "not here" chosen by the caller.
    fn run_answering(
        dir: &str,
        args: &[&str],
        not_here: impl Fn(&str) -> bool,
    ) -> Result<Option<Said>> {
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
        if output.status.code() == Some(GIT_FATAL) && not_here(&stderr) {
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

/// Whether git's refusal says the path is in no repository: [`NOT_A_REPOSITORY`], or
/// [`CANNOT_CHANGE_TO`] whole.
fn no_repository_here(stderr: &str) -> bool {
    let (cannot_change_to, not_there) = CANNOT_CHANGE_TO;
    stderr.contains(NOT_A_REPOSITORY)
        || stderr
            .lines()
            .any(|line| line.starts_with(cannot_change_to) && line.ends_with(not_there))
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
/// symref — is not here and cannot be: the walk exits 0 with nothing said. Under the
/// unreadable directory a ref `packed-refs` also holds is not even dropped: it is listed at
/// the packed value, out nowhere. Issue #32 asks the checkout instead, where one has the ref
/// out: [`GitPort::has_no_commit_yet`]. That catches the unreadable directory where herdr
/// lists the checkout as not detached (`is_detached: false`) and no ref of the repository
/// names its path, and not the dangling symref, which git answers for as a branch with no
/// commit yet.
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

/// Whether `line` reads as `perror`'s report of a path git could not `lstat`: something, `: `,
/// and then the C library's words for why — which start with a capital letter and have no
/// `: ` of their own — on a line that does not open `warning: `, `fatal: ` or `error: cannot
/// run `, the openings of git's own measured ending the same way. See `unread_paths` for
/// what that takes in and what it leaves out.
fn lstat_failed(line: &str) -> bool {
    !["warning: ", "fatal: ", "error: cannot run "]
        .iter()
        .any(|word| line.starts_with(word))
        && line.rsplit_once(": ").is_some_and(|(path, said)| {
            !path.is_empty() && said.starts_with(|c: char| c.is_ascii_uppercase())
        })
}

/// What git said while leaving part of a working tree out of `status`, or nothing.
///
/// Three shapes, measured against git 2.55.0, all on an exit of 0.
///
/// A directory git cannot open gives `warning: could not open directory 'notes/': Permission
/// denied`, once per directory, and nothing under it is listed — tracked or untracked. Read
/// by its opening, whatever the reason after it, bar one: `No such file or directory` is a
/// directory git saw when it listed its parent and that was gone by the time it opened it —
/// measured, with a tool creating and removing an untracked directory beside a clean
/// checkout, git says so on a share of its calls. Nothing was there to leave out, and read
/// as a partial answer the row would flicker to `?`.
///
/// A file of ignore rules git cannot read — a `.gitignore`, `.git/info/exclude`, the file
/// `core.excludesFile` names — gives `warning: unable to access '<file>': Permission denied`,
/// sometimes more than once, and git answers as though the file were empty. A
/// `.gitattributes` or `.git/info/attributes` gives the same words, but only when `status`
/// had to compare a tracked file's content to answer; otherwise git never opens it and says
/// nothing. For ignore rules an empty file mostly lists more, not less; but a `!` rule
/// re-includes what an earlier rule excluded, and with it unread the file it named is left
/// out — measured, a checkout whose only work was such a file read as clean, and `git
/// worktree remove` deleted it with an exit of 0. git read the tree by rules it could not
/// see, so the answer is a partial one whichever way it fell, and it is read by its opening
/// too. The cost is a `core.excludesFile` git cannot read: every repository that uses it
/// reads it, so every row of those says `?`. Those rules come last and so, unread, can only
/// list more — measured — but telling that file from the others would take another call to
/// git per checkout, and `dump` names it.
///
/// Not when the reason is `Too many levels of symbolic links`. git does not follow a
/// `.gitignore` or `.gitattributes` in the working tree that is a symlink, and says so in
/// those words — measured, a tracked `.gitignore` linked to a readable file gets the warning
/// on every `status`, and its rules are not applied. That is git's rule rather than a file it
/// could not reach, and the answer is whole by it; read as one, every row of such a
/// repository would say `?` for good. A symlink that really does loop gives the same words,
/// and so does a chain of symlinks longer than the system follows — that one with a file at
/// its end, whose rules go unseen: a `!` rule there would hide what it re-included.
///
/// A tracked file git cannot `lstat` gives its path, `: `, and the C library's words, with no
/// prefix — `perror` in git's `diff-lib.c` — and is left out of the answer. In a directory
/// git may list but not search (mode `r--`) that holds only tracked files it is the only
/// line, and a modified file there was read as a clean working tree. Where a symlink that
/// loops stands in for a directory of tracked files and is itself ignored, it is `t/f: Too
/// many levels of symbolic links`, and the file's absence read as clean where a plain
/// deletion is ` D`. It also appears beside the first warning, for a tracked file under a
/// directory that could not be opened.
///
/// Read whatever the reason, so long as the line has that shape (see [`lstat_failed`]): an
/// `lstat` can fail in ways nobody here has measured — a sandbox refusing a file gives `t/f:
/// Operation not permitted`, measured — and a reason missed is a modified file read as
/// clean, which a sweep would act on. Reading the reason after the last `: ` rather than the
/// first is what keeps a tracked path with `: ` in its name. Lines of git's own that end the
/// same way — `fatal: cannot exec '<hook>': Permission denied` from a `core.fsmonitor` hook
/// that cannot run, `error: cannot run <command>: No such file or directory` from one named
/// by a command that is not on the path, `warning: could not open '<file>' for tracing:
/// Permission denied` — are not read; after each git looked at everything itself. The lines
/// `GIT_TRACE`, `GIT_TRACE_SETUP` and `GIT_TRACE_PERFORMANCE` write have nothing starting
/// with a capital letter after their last `: ` when git runs at a checkout's root, as it does
/// here — measured — and are not read either. `GIT_TRACE2` is different: it repeats each of
/// git's own errors without its opening, so with it on stderr the lines above are read — `?`
/// where git's answer was whole, the safe side. The cost
/// is a hook that runs and fails inside itself: its own `<hook>: line 2: <tool>: Permission
/// denied` reads as one, and the row says `?` though git's answer was whole — the safe side,
/// for a case that needs a broken hook to happen.
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
/// In git's English, which git translates and [`GIT_LOCALE`] is what keeps it in. The words
/// after the last colon are the C library's, not git's: `LC_ALL=C` keeps them English where
/// the library translates them at all (glibc ships translations; the macOS one measured does
/// not translate them). Three shapes rather than "stderr said something", for the reason
/// `dropped_refs` gives: a trace variable writes to stderr on every call. Every matching line
/// is kept — a line repeated straight after itself only once — and not passed through
/// [`refusal`], for the reason `dropped_refs` gives first: it is always this call, so naming
/// it tells a reader nothing they could act on.
fn unread_paths(stderr: &str) -> Option<String> {
    let mut unread: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| {
            (line.starts_with("warning: could not open directory ")
                && !line.ends_with(": No such file or directory"))
                || (line.starts_with("warning: unable to access ")
                    && !line.ends_with(": Too many levels of symbolic links"))
                || lstat_failed(line)
        })
        .collect();
    unread.dedup();
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
/// [`app::collect::identify_one`](crate::app::collect) keeps these words apart from git
/// answering that a path is outside a repository, so with no git at all the picker says why
/// the panes it could not place are ungrouped rather than leaving the heading to be read as
/// herdr's failure.
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
        let Some(paths) = GitCli::run_answering(
            cwd,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
                "--show-toplevel",
            ],
            |stderr| no_repository_here(stderr) || stderr.contains(NOT_A_WORK_TREE),
        )?
        else {
            return Ok(None);
        };
        let mut lines = paths.stdout.lines();
        let (Some(repo_key), Some(checkout_path)) = (lines.next(), lines.next()) else {
            return Ok(None);
        };

        // `symbolic-ref` rather than `rev-parse --abbrev-ref`: it names the branch of a HEAD
        // with no commit yet — a new repository, a `worktree add --orphan` — where rev-parse
        // exits 128, and that exit would fail the whole identity above with it. A detached
        // HEAD makes it exit 1 without a word, which is "no branch".
        // swallows: any other failure to name the branch. The identity is what the caller
        // needs; the branch rides along, and no caller reads it to place a pane.
        let branch = GitCli::command(cwd)
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|name| !name.is_empty());

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
        // A clean exit is not a whole answer here either. A directory git could not open, or
        // a tracked file it could not look at, is reported on stderr and simply absent from
        // stdout — and an ignore file it could not read can leave out what it re-included —
        // so an empty stdout beside those words is not `clean`: it is the answer git could
        // not give, about the one working tree a sweep would then act on. `Err`, because a
        // `bool` has nowhere to carry the words; `app::dirty` reads it as
        // `WorkingTree::Unreadable`, and `dump` asks again and prints what git says then.
        //
        // Whatever stdout holds, not only when it is empty. A checkout can have visible work
        // *and* a part git could not look at, and `true` would then be a whole answer to
        // the question asked — but it is a whole answer arrived at by accident, from the
        // half git could see. A partial reading is not reported as a complete one because
        // the visible half happened to land on the safe side; the row says `?` rather than
        // `✱`, and both refuse a sweep.
        if let Some(words) = unread_paths(&status.stderr) {
            bail!("{words}");
        }
        Ok(!status.stdout.trim().is_empty())
    }

    fn has_no_commit_yet(&self, checkout_path: &str) -> Result<bool> {
        let args = ["log", "-1", "--format=%H"];
        let output = GitCli::output(checkout_path, &args)?;
        if output.status.success() {
            return Ok(false);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let (exit, said) = NO_COMMITS_YET;
        if output.status.code() == Some(exit) && stderr.contains(said) {
            return Ok(true);
        }
        bail!("{}", refusal(&args, &stderr));
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
        could_not_run, dropped_refs, github_slug_from_url, no_repository_here, no_such_remote,
        parse_track, read_track, refusal, unread_paths, GitCli, Slug, GIT_LOCALE,
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
    fn only_the_words_of_a_partial_status_are_read_as_unread() {
        // The same rule as for a dropped ref: stderr is where git puts everything that is
        // not an answer, so three shapes and not "stderr said something".
        assert_eq!(
            unread_paths("warning: could not open directory 'notes/': Permission denied\n")
                .as_deref(),
            Some("warning: could not open directory 'notes/': Permission denied")
        );
        for said in [
            "",
            // A trace variable's lines, which end in no capital after their last `: `.
            "11:35:24.293334 git.c:502  trace: built-in: git status\n",
            "08:51:45.407752 trace.c:316             setup: git_dir: .git\n",
            "08:51:45.408105 trace.c:320             setup: prefix: (null)\n",
            "08:51:45.418919 read-cache.c:2364       performance: 0.000026000 s:  read cache .git/index\n",
            "warning: ignoring broken ref refs/heads/wip\n",
            // A warning on a clean exit that leaves nothing out of the answer.
            "warning: core.fsyncObjectFiles is deprecated; use core.fsync instead\n",
            // A `core.fsmonitor` hook that cannot run, by path or by a command not on the
            // path: git looks at everything itself.
            "fatal: cannot exec '.git/hooks/fsmonitor-watchman': Permission denied\n",
            "error: cannot run no-such-fsmonitor: No such file or directory\n",
            // A trace target git cannot open, said on every call.
            "warning: could not open '/ro/trace' for tracing: Permission denied\n",
            // A `.gitignore` in the tree that is a symlink, which git does not follow.
            "warning: unable to access '.gitignore': Too many levels of symbolic links\n",
            // A directory gone between git listing its parent and opening it.
            "warning: could not open directory 'tmp/': No such file or directory\n",
        ] {
            assert_eq!(unread_paths(said), None, "not a partial answer: {said:?}");
        }

        // The two warnings are read by their opening, whatever the reason after it.
        for said in [
            "warning: could not open directory 'x/': Too many open files",
            "warning: unable to access 'x/.gitignore': Input/output error",
        ] {
            assert_eq!(unread_paths(said).as_deref(), Some(said));
        }

        // `symbolic` or `No such file` elsewhere than the reason does not excuse a line.
        for said in [
            "warning: unable to access 'symbolic/.gitignore': Permission denied",
            "warning: could not open directory 'No such file or directory/': Permission denied",
        ] {
            assert_eq!(unread_paths(said).as_deref(), Some(said));
        }

        // `perror`'s line, whatever the reason: each way `lstat` failed that was measured,
        // one nobody measured, a path with `: ` in its name — and a hook's own failing line,
        // which is the same shape and is read the same way.
        for said in [
            "t/f: Permission denied",
            "t/f: Too many levels of symbolic links",
            "t/f: File name too long",
            "t/f: Operation not permitted",
            "t/f: Input/output error",
            "t/a: b: Permission denied",
            ".git/hooks/fsm: line 2: /usr/bin/tool: Permission denied",
        ] {
            assert_eq!(unread_paths(said).as_deref(), Some(said));
        }

        // A tracked file git could not `lstat`, in a directory it may list but not search
        // that holds nothing else: then this line is the whole of what git says.
        assert_eq!(
            unread_paths("t/f: Permission denied\n").as_deref(),
            Some("t/f: Permission denied")
        );

        // A file of rules git could not read, which git says more than once for some of
        // them, one line straight after another: said once.
        assert_eq!(
            unread_paths(
                "warning: unable to access '.git/info/exclude': Permission denied\n\
                 warning: unable to access '.git/info/exclude': Permission denied\n"
            )
            .as_deref(),
            Some("warning: unable to access '.git/info/exclude': Permission denied")
        );

        // Both lines git prints for a tracked file under a directory it could not open. Both
        // are kept, in git's order.
        assert_eq!(
            unread_paths(
                "deep/inner/u: Permission denied\n\
                 warning: could not open directory 'deep/inner/': Permission denied\n"
            )
            .as_deref(),
            Some(
                "deep/inner/u: Permission denied \
                 warning: could not open directory 'deep/inner/': Permission denied"
            )
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
    fn a_directory_git_could_not_enter_is_no_repository_only_when_it_is_not_there() {
        // The first three are what git 2.55.0 says with `-C` pointed at a directory outside
        // any repository, one that is not there, and one it may not enter; the rest are
        // written for what must not match.
        assert!(no_repository_here(
            "fatal: not a git repository (or any of the parent directories): .git\n"
        ));
        assert!(no_repository_here(
            "fatal: cannot change to '/wt/gone': No such file or directory\n"
        ));
        // A directory that is there and git could not enter. git never looked, so this is
        // not an answer about a repository.
        assert!(!no_repository_here(
            "fatal: cannot change to '/wt/locked': Permission denied\n"
        ));
        assert!(!no_repository_here(
            "fatal: cannot change to '/mnt/share': Stale NFS file handle\n"
        ));
        // The ending on a line about something else is not about the path git was pointed at.
        assert!(!no_repository_here(
            "fatal: could not open '/src/app/.git/HEAD': No such file or directory\n"
        ));
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
