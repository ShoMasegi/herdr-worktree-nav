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

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde::Deserialize;

use crate::port::{
    GhPort, PullRequest, PullRequestOutcome, SettledPullRequest, SettledPullRequests, Slug,
};

/// More open pull requests than anyone scrolls through in a branch picker.
const LIMIT: &str = "100";

/// A wider window than the open list, because it looks back over everything that has landed
/// rather than at what is in flight. A repository busier than this is not told "no pull
/// request" for the branches beyond it — see `SettledPullRequests::Window`.
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
///
/// Owned strings and one argument, at the cost of ten allocations per sweep. One argument of
/// one type cannot be handed its arguments the wrong way round, which is what the two-argument
/// version could be. `--limit` is read from `SETTLED_LIMIT` here rather than passed in, which
/// removes the *pair* — but not the second mention of the constant, which is in
/// [`settled_answer`]. Those two still have to agree and only a test says they do: a window
/// measured smaller than the one asked for makes every non-empty answer look truncated, and
/// `domain::sweep` then calls every clean named branch `Unjudged`.
fn settled_arguments(slug: &Slug) -> [String; 10] {
    [
        "pr".to_string(),
        "list".to_string(),
        "-R".to_string(),
        slug.as_str().to_string(),
        "--state".to_string(),
        "closed".to_string(),
        "--limit".to_string(),
        SETTLED_LIMIT.to_string(),
        "--json".to_string(),
        "number,headRefName,isCrossRepository,state".to_string(),
    ]
}

/// What `gh` prints for `--json state`. Anything that is not one of these is a state this
/// does not know, and an unknown state is not a licence to offer a deletion.
#[derive(Deserialize)]
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
/// Separate from running the command so that what `gh` said can be tested without a network,
/// a token, or a `gh` on the machine — whether an answer is the whole answer, and what each
/// field means once it is here. What the command *asks* is the other half, and it is where
/// this call's two shipped bugs both lived: see `settled_arguments`.
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

/// The decoration query, made but not run. `stderr` goes to `null` rather than being piped
/// because nothing reads it: ADR 0003 says a missing `gh` costs the annotation and nothing
/// else here, so there is no sentence to show and no half of the answer to name.
///
/// Split out for the reason the sweep's is, one bug earlier: this is where a checkout path
/// was passed to `-R` first, and it stayed wrong for as long as it did because a picker that
/// silently draws no annotations looks exactly like a repository with no pull requests.
fn open_command(slug: &Slug) -> Command {
    let mut command = Command::new("gh");
    command
        .args([
            "-R",
            slug.as_str(),
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            LIMIT,
            "--json",
            "number,title,headRefName,isDraft",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// The process, made but not run.
///
/// The third thing split out of this call, and for the reason the first two were: what is
/// left inside `settled_pull_requests` is `.output()` and nothing else. Both bugs this call
/// has shipped were in what it asked, and what it asks is now pinned — the program and the
/// argument list are both assertable, and asserted.
///
/// The redirections are not. `Command` has getters for the program, the arguments, the
/// environment and the working directory, and none for `stdin`/`stdout`/`stderr`, so
/// `.stderr(Stdio::null())` here would go unnoticed by any test that does not start a real
/// `gh`. What that costs is not nothing: [`settled_answer`] reads `stderr` to find the
/// sentence the user is shown, so a `stderr` sent to `null` makes every word of it
/// unreachable and turns every refusal into "gh would not answer". Reaching it needs a `gh`
/// on `PATH` that a test put there.
fn settled_command(slug: &Slug) -> Command {
    let mut command = Command::new("gh");
    command
        .args(settled_arguments(slug))
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    command
}

/// What one run of the decoration query amounts to. `settled_answer`'s twin, and the half of
/// this adapter that ships today.
///
/// Every way this can come back empty means the same thing here — no `gh`, a `gh` that
/// refused, output in a shape this cannot read — and all three are the annotation costing
/// nothing, which is ADR 0003's promise. That is the opposite of the sweep's rule and it is
/// why they are two functions: a sweep must say which half it could not see, and a branch
/// list must never make the user's `gh` its problem.
///
/// Extracted because the argument list is not the only thing here that a green suite proved
/// nothing about. Dropping the exit check, or asking for fewer `--json` fields, or swapping
/// two `serde` renames, all left the whole gate green — and the second of those draws every
/// row's branch and draft flag from the wrong field.
fn open_answer(output: &Output) -> Vec<PullRequest> {
    if !output.status.success() {
        return Vec::new();
    }
    let parsed: Vec<GhPullRequest> = serde_json::from_slice(&output.stdout).unwrap_or_default();
    parsed
        .into_iter()
        .map(|pull_request| PullRequest {
            number: pull_request.number,
            title: pull_request.title,
            head_ref: pull_request.head_ref_name,
            is_draft: pull_request.is_draft,
        })
        .collect()
}

/// What one run of `gh` amounts to: an answer, or a sentence saying why there is not one.
///
/// Separate from starting the process for the same reason [`read_settled`] is separate from
/// this — everything above `Command::new` can then be tested, and this half decides three
/// things a green suite otherwise proved nothing about: that a `gh` which exited non-zero is
/// not read as an answer, that the window truncation is measured against the window that was
/// asked for — both directions, since only a *smaller* one does damage — and which of `gh`'s
/// own words the user is shown.
fn settled_answer(output: &Output) -> Result<SettledPullRequests, String> {
    if !output.status.success() {
        // `gh`'s own words, because the alternative is a picker that says a sweep could not
        // look and cannot say why. Trimmed to the first line with anything on it — `gh`
        // prefixes warnings and blank lines of its own, and the first line being one of
        // those is exactly when the reason exists and is on the second. The rest is usually
        // a usage dump, and this ends up on one prompt line.
        let said = String::from_utf8_lossy(&output.stderr);
        let reason = said.lines().map(str::trim).find(|line| !line.is_empty());
        return Err(match reason {
            // Not "gh: …". The two bugs this call has already had were malformed argv on
            // this side, and both read to a user as "your gh is broken" — which is the one
            // thing this cannot tell apart from GitHub saying no.
            Some(reason) => format!("gh refused the question this asked: {reason}"),
            None => format!("gh would not answer ({})", output.status),
        });
    }
    read_settled(&output.stdout, SETTLED_LIMIT)
}

pub struct GhCli;

impl GhPort for GhCli {
    fn pull_requests(&self, slug: &Slug) -> Vec<PullRequest> {
        let Ok(output) = open_command(slug).output() else {
            return Vec::new();
        };
        open_answer(&output)
    }

    fn settled_pull_requests(&self, slug: &Slug) -> Result<SettledPullRequests, String> {
        let output = settled_command(slug)
            .output()
            .map_err(|error| format!("gh could not be run: {error}"))?;
        settled_answer(&output)
    }
}

/// How long a caller should wait for `gh` before giving up on the annotation.
pub const GH_BUDGET: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    fn slug() -> Slug {
        Slug::owner_repo("me", "app").expect("two names GitHub would know")
    }

    #[test]
    fn the_repository_is_named_rather_than_guessed_from_the_remotes() {
        // Twice wrong here. `-R` was given a filesystem path, which it rejects outright, so
        // every call failed on every machine. Dropping the flag fixed that and introduced a
        // quieter fault: `gh` then picks a base repository out of the remotes, and for a
        // fork it picks the parent — answering about somebody else's repository with a zero
        // exit. A whole green suite noticed neither, because nothing else in it runs `gh`.
        // Pinning the argument list is the cheapest thing that would have — and the third
        // time it shipped, in `app::branches`, the argument list was right and the call site
        // was not, which is why `Slug` is a type rather than a convention.
        let arguments = settled_arguments(&slug());
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
    fn what_is_actually_started_is_gh_with_the_arguments_above() {
        // The list being right has never been the whole of it: twice a correct list sat
        // beside a call that did not use it. What is left in `settled_pull_requests` after
        // this is `.output()`, and `stderr` is piped because `settled_answer` reads it — a
        // `stderr` sent to `null` costs the user gh's own words on every refusal without
        // failing anything.
        let command = settled_command(&slug());
        assert_eq!(command.get_program(), "gh");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            settled_arguments(&slug())
                .iter()
                .map(std::ffi::OsStr::new)
                .collect::<Vec<_>>(),
            "the arguments this asks for are the arguments it is started with"
        );
    }

    #[test]
    fn the_decoration_query_names_the_repository_too() {
        // Where the path-for-slug bug was first written. It never failed loudly: `gh` exits
        // non-zero, `pull_requests` turns that into an empty list by design, and a picker
        // drawing no annotations looks like a repository with no open pull requests. Nothing
        // in this module was tested at all until #23, which is why it stayed.
        let command = open_command(&slug());
        assert_eq!(command.get_program(), "gh");
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            arguments.windows(2).any(|pair| pair == ["-R", "me/app"]),
            "the repository is named, not guessed from the remotes: {arguments:?}"
        );
        assert!(
            arguments.windows(2).any(|pair| pair == ["--state", "open"]),
            "and this is the half in flight, not the half the sweep asks about"
        );
        // The whole list, for the reason the sweep's is asserted whole: a field dropped from
        // `--json` costs a column the row draws and nothing else notices. `isDraft` is the
        // one that dims a row, `headRefName` is what matches it to a branch at all.
        assert_eq!(
            arguments,
            [
                "-R",
                "me/app",
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                LIMIT,
                "--json",
                "number,title,headRefName,isDraft",
            ]
        );
    }

    #[test]
    fn the_window_asked_for_is_the_window_truncation_is_measured_against() {
        // Two numbers that must agree: what `gh` is told to return, and how many coming back
        // means there may be more behind them. They agree by being one constant read twice
        // rather than one value passed twice, so this asserts the reading rather than the
        // passing — the argument that could have been given the wrong number is gone.
        let arguments = settled_arguments(&slug());
        let asked_for = arguments
            .iter()
            .position(|argument| argument == "--limit")
            .map(|at| arguments[at + 1].as_str());
        assert_eq!(asked_for, Some(SETTLED_LIMIT.to_string().as_str()));
    }

    #[test]
    fn closed_is_asked_for_rather_than_everything() {
        // `--state all` returns open pull requests too, counted against the same window and
        // then thrown away here — so a busy repository spends its whole window on the
        // answers this does not want and truncates away the ones it does.
        let arguments = settled_arguments(&slug());
        let state = arguments
            .iter()
            .position(|argument| argument == "--state")
            .map(|at| arguments[at + 1].as_str());
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

/// What one run of `gh` amounts to, over an `Output` this makes rather than one `gh` made.
///
/// The half of this adapter that had no tests at all. `read_settled` was carefully pinned
/// and `settled_arguments` exactly so, and between them sat a runner where a `gh` that
/// exited non-zero could be parsed as an answer, the window could be measured against a
/// different number from the one asked for, and the sentence shown to the user could be
/// thrown away — none of it observable, because nothing in the suite runs `gh`.
#[cfg(test)]
mod answering {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn ran(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            // The raw wait status, which is the exit code shifted up a byte.
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn the_decoration_reads_every_field_it_asked_for() {
        // Nothing else in the suite ever read this method's output. Dropping the exit check,
        // asking for fewer `--json` fields, or swapping two `serde` renames all left the
        // whole gate green — and the last of those draws every row's branch name and draft
        // flag out of the wrong field, on the picker that ships today.
        let listed = open_answer(&ran(
            0,
            r#"[{"number":7,"title":"Add a sweep","headRefName":"feat/sweep","isDraft":true}]"#,
            "",
        ));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].number, 7);
        assert_eq!(listed[0].title, "Add a sweep");
        assert_eq!(listed[0].head_ref, "feat/sweep");
        assert!(
            listed[0].is_draft,
            "the flag the row draws its dimming from"
        );
    }

    #[test]
    fn a_gh_that_refused_costs_the_decoration_and_nothing_else() {
        // The opposite rule to the sweep's, and the reason these are two functions. ADR 0003
        // says a missing or unhappy `gh` costs the annotation here and nothing more, so all
        // three ways of coming back empty are the same answer — but the exit check is what
        // makes the first of them empty rather than whatever a refusal left on stdout.
        assert!(
            open_answer(&ran(
                1,
                r#"[{"number":7,"title":"t","headRefName":"b","isDraft":false}]"#,
                ""
            ))
            .is_empty(),
            "a gh that exited non-zero has not answered, whatever is on its stdout"
        );
        assert!(
            open_answer(&ran(0, "not json at all", "")).is_empty(),
            "and output this cannot read is the same cost"
        );
    }

    #[test]
    fn a_gh_that_refused_is_not_read_as_an_answer() {
        // The conflation ADR 0011 exists to prevent, in its cheapest form: `gh` prints
        // nothing to stdout when it refuses, and an empty stdout parses as an empty list —
        // so reading it anyway turns "could not look" into "nothing is finished with", and
        // a sweep offers nothing while saying it looked.
        let refused = settled_answer(&ran(1, "[]", "could not resolve to a Repository\n"));
        assert!(refused.is_err(), "an exit code is not a shape of answer");
        assert!(
            settled_answer(&ran(0, "[]", "")).is_ok(),
            "and an empty answer from a gh that succeeded still is one"
        );
    }

    #[test]
    fn gh_is_quoted_rather_than_blamed() {
        let said = settled_answer(&ran(1, "", "unknown flag: --stat\n")).unwrap_err();
        assert!(
            said.contains("unknown flag: --stat"),
            "gh's own words are the only thing that says which half went wrong: {said}"
        );
        assert!(
            !said.starts_with("gh:"),
            "and the two bugs this call has had were on this side, not gh's: {said}"
        );
    }

    #[test]
    fn the_reason_is_found_past_the_blank_lines_and_no_further() {
        // Taking the first line outright shows the user a blank sentence at the moment there
        // is something to say, because `gh` puts a newline of its own ahead of some errors.
        // Blank is all this skips. A `gh` with an update notice to deliver leads with that,
        // and then the update notice is what the user is told the sweep failed on — which is
        // wrong, and is not fixed by guessing at which prefixes are notices. It needs a `gh`
        // on `PATH` that a test put there, which is the same thing the redirections need;
        // see `settled_command`.
        let said = settled_answer(&ran(1, "", "\n  \nGraphQL: Could not resolve\n")).unwrap_err();
        assert!(
            said.contains("Could not resolve"),
            "the first line with anything on it, not the first line: {said}"
        );

        let notice = settled_answer(&ran(
            1,
            "",
            "A new release of gh is available\nGraphQL: Could not resolve\n",
        ))
        .unwrap_err();
        assert!(
            notice.contains("A new release of gh is available"),
            "and it goes no further than blank, which this says rather than hides: {notice}"
        );
    }

    #[test]
    fn a_gh_that_said_nothing_still_says_which_gh_it_was() {
        let said = settled_answer(&ran(2, "", "   \n\n")).unwrap_err();
        assert!(
            said.contains("gh would not answer"),
            "an empty stderr is not a reason to show an empty sentence: {said}"
        );
    }

    #[test]
    fn the_window_gh_was_asked_for_is_the_one_a_full_answer_is_measured_against() {
        // `read_settled`'s truncation rule is tested over a limit a test chose. This is the
        // limit production passes, and passing a bigger one would mean no answer is ever
        // `Window` — a busy repository told "no pull request" for every branch past the
        // window, with nothing on screen saying the window ran out.
        let full: String = (1..=SETTLED_LIMIT)
            .map(|number| {
                format!(r#"{{"number":{number},"headRefName":"b{number}","isCrossRepository":false,"state":"MERGED"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        let answer = settled_answer(&ran(0, &format!("[{full}]"), "")).unwrap();
        assert!(
            matches!(answer, SettledPullRequests::Window(_)),
            "as many as were asked for is the only evidence gh gives that there may be more"
        );

        // And the other direction, which is the one that does damage. A window measured
        // *smaller* than the one asked for makes every non-empty answer look truncated, and
        // `domain::sweep` calls every clean named branch `Unjudged` — the whole list saying
        // "could not judge" on a repository where nothing went wrong.
        let one_short = full[..full.rfind(",{").expect("more than one")].to_string();
        let answer = settled_answer(&ran(0, &format!("[{one_short}]"), "")).unwrap();
        assert!(
            matches!(answer, SettledPullRequests::All(_)),
            "one fewer than the window is gh saying it reached the end"
        );
    }
}
