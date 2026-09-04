//! What a finished removal says, and to whom.
//!
//! A removal runs in a process of its own so that it outlives the picker — see
//! `docs/adr/0014-removing-outlives-the-picker.md`. That leaves two readers to serve and one
//! set of words to serve them with. The toast is the report that always happens, because it
//! is the only one left when the picker has already closed. The prompt line is the extra
//! the picker adds when it is still up to read the answer.

use crate::port::{Notification, NotificationSound, RemovalOutcome};

/// What a report line starts with when the checkout went.
const REMOVED: &str = "removed";
/// What it starts with when git declined, followed by git's own words.
const REFUSED: &str = "refused ";

/// The one line a detached removal writes for whoever started it.
///
/// It is the only channel between the two processes and it has to survive the reader having
/// walked away, so it is deliberately trivial: one line, no framing, nothing to get out of
/// step. git's message is folded onto that line because a toast shows it on one anyway.
pub fn report_line(outcome: &RemovalOutcome) -> String {
    match outcome {
        RemovalOutcome::Removed => REMOVED.to_string(),
        RemovalOutcome::Refused(reason) => {
            let folded: Vec<&str> = reason.lines().map(str::trim).collect();
            format!("{REFUSED}{}", folded.join(" "))
        }
    }
}

/// Read a report line back. `None` for anything else on that channel — a line the picker
/// cannot read is not an outcome, and guessing that it meant success is how a checkout that
/// is still there stops being reported.
pub fn parse_report(line: &str) -> Option<RemovalOutcome> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line == REMOVED {
        return Some(RemovalOutcome::Removed);
    }
    line.strip_prefix(REFUSED)
        .map(|reason| RemovalOutcome::Refused(reason.to_string()))
}

/// The toast a finished removal shows.
///
/// It names the branch rather than the plugin, because the branch is what the reader was
/// waiting on. The path goes in the body: it is what actually went, and it is what tells two
/// checkouts of the same branch name apart.
///
/// `panes_closed` is how many panes were stopped to get this far, and it appears only when
/// the removal was then refused — see [`refusal`].
pub fn notification(
    label: &str,
    checkout_path: &str,
    outcome: &RemovalOutcome,
    panes_closed: usize,
) -> Notification {
    match outcome {
        RemovalOutcome::Removed => Notification {
            title: format!("removed {label}"),
            body: Some(checkout_path.to_string()),
            // Tidying up is done often, and a chime for every checkout that goes is noise.
            sound: NotificationSound::None,
        },
        RemovalOutcome::Refused(reason) => Notification {
            title: format!("could not remove {label}"),
            // git's words, not a summary of them: they say what would have been lost.
            body: Some(refusal(reason, panes_closed)),
            // The one that has to reach someone who is no longer looking.
            sound: NotificationSound::Request,
        },
    }
}

/// git's refusal, and what it cost to reach it.
///
/// A removal that stopped panes and then failed is the one failure that is not "nothing
/// happened": the panes are gone and the checkout is not. Saying only what git said would
/// leave the reader to work that out from an empty tab. Nothing is added when the removal
/// worked — the panes were named in the question, and the checkout going is the answer.
fn refusal(reason: &str, panes_closed: usize) -> String {
    match panes_closed {
        0 => reason.to_string(),
        1 => format!("{reason} — its 1 pane was closed first"),
        many => format!("{reason} — its {many} panes were closed first"),
    }
}

/// What the picker puts on its prompt line, when it is still up to read the answer.
///
/// Nothing on success: the row leaving the list is the report, and repeating it there would
/// only say twice what the toast has already said once.
pub fn message(label: &str, outcome: &RemovalOutcome, panes_closed: usize) -> Option<String> {
    match outcome {
        RemovalOutcome::Removed => None,
        // Several removals can be in flight at once, so the reason has to name its own.
        RemovalOutcome::Refused(reason) => Some(format!(
            "could not remove {label}: {}",
            refusal(reason, panes_closed)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{NotificationSound, RemovalOutcome};

    const REFUSAL: &str = "`git worktree remove /wt/fix-crash` failed: fatal: '/wt/fix-crash' \
                           contains modified or untracked files, use --force to delete it";

    #[test]
    fn a_removal_that_worked_survives_the_trip_between_the_processes() {
        let line = report_line(&RemovalOutcome::Removed);
        assert_eq!(parse_report(&line), Some(RemovalOutcome::Removed));
    }

    #[test]
    fn a_refusal_carries_gits_words_across_unchanged() {
        let line = report_line(&RemovalOutcome::Refused(REFUSAL.to_string()));
        assert_eq!(
            parse_report(&line),
            Some(RemovalOutcome::Refused(REFUSAL.to_string()))
        );
    }

    #[test]
    fn a_refusal_that_spans_lines_still_travels_as_one() {
        let line = report_line(&RemovalOutcome::Refused(
            "fatal: one\nfatal: two".to_string(),
        ));
        assert_eq!(line.lines().count(), 1, "the channel is a single line");
        assert_eq!(
            parse_report(&line),
            Some(RemovalOutcome::Refused("fatal: one fatal: two".to_string()))
        );
    }

    #[test]
    fn anything_else_on_that_channel_is_not_an_outcome() {
        // A line the picker cannot read is not a removal it can report on. Saying so beats
        // guessing that silence meant success.
        assert_eq!(parse_report(""), None);
        assert_eq!(parse_report("Killed"), None);
    }

    #[test]
    fn a_refusal_after_panes_were_closed_says_that_they_were() {
        // The one case where a failure is not simply "nothing happened": the panes are
        // already gone by the time git speaks, and a report that only quoted git would
        // leave the user to work that out from an empty tab.
        let notification = notification(
            "fix/crash",
            "~/.herdr/worktrees/app/fix-crash",
            &RemovalOutcome::Refused(REFUSAL.to_string()),
            2,
        );
        assert_eq!(
            notification.body.as_deref(),
            Some(format!("{REFUSAL} — its 2 panes were closed first").as_str())
        );
        assert_eq!(
            message(
                "fix/crash",
                &RemovalOutcome::Refused(REFUSAL.to_string()),
                1
            ),
            Some(format!(
                "could not remove fix/crash: {REFUSAL} — its 1 pane was closed first"
            ))
        );
    }

    #[test]
    fn a_removal_that_worked_does_not_dwell_on_the_panes() {
        // They were listed in the question and the checkout is gone; saying it again is
        // saying twice what the row leaving the list already said once.
        let notification = notification(
            "fix/crash",
            "~/.herdr/worktrees/app/fix-crash",
            &RemovalOutcome::Removed,
            2,
        );
        assert_eq!(
            notification.body.as_deref(),
            Some("~/.herdr/worktrees/app/fix-crash")
        );
        assert_eq!(message("fix/crash", &RemovalOutcome::Removed, 2), None);
    }

    #[test]
    fn the_toast_names_the_branch_and_points_at_the_path() {
        let notification = notification(
            "fix/crash",
            "~/.herdr/worktrees/app/fix-crash",
            &RemovalOutcome::Removed,
            0,
        );
        assert_eq!(notification.title, "removed fix/crash");
        assert_eq!(
            notification.body.as_deref(),
            Some("~/.herdr/worktrees/app/fix-crash")
        );
        assert_eq!(
            notification.sound,
            NotificationSound::None,
            "tidying up is done often; a chime every time would be noise"
        );
    }

    #[test]
    fn the_refused_one_is_the_one_that_makes_a_sound() {
        let notification = notification(
            "fix/crash",
            "~/.herdr/worktrees/app/fix-crash",
            &RemovalOutcome::Refused(REFUSAL.to_string()),
            0,
        );
        assert_eq!(notification.title, "could not remove fix/crash");
        assert_eq!(
            notification.body.as_deref(),
            Some(REFUSAL),
            "git's words, not a summary of them"
        );
        assert_eq!(notification.sound, NotificationSound::Request);
    }

    #[test]
    fn the_picker_says_nothing_when_the_row_simply_leaves() {
        assert_eq!(message("fix/crash", &RemovalOutcome::Removed, 0), None);
    }

    #[test]
    fn the_picker_repeats_the_refusal_and_says_which_checkout_it_was_about() {
        // Several removals can be in flight at once, so the reason has to name its own.
        assert_eq!(
            message(
                "fix/crash",
                &RemovalOutcome::Refused(REFUSAL.to_string()),
                0
            ),
            Some(format!("could not remove fix/crash: {REFUSAL}"))
        );
    }
}
