//! The panes picker: draw, read a key, and act on what it meant.

use anyhow::Result;
use ratatui::crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::app::collect;
use crate::app::dirty::Dirty;
use crate::app::home_dir;
use crate::app::removals::Removals;
use crate::app::Pending;
use crate::domain::removal::{self, Removal, SweepRemoval};
use crate::port::{GitPort, HerdrPort, PaneSplit, SplitDirection, WorktreeOpen};
use crate::ui::render::{self, Mode};
use crate::ui::state::{Action, Cancelled, PanesState, WITHDRAWN};
use crate::ui::theme::Theme;

/// How long to wait for a key before turning the spinner on whatever is still coming. The
/// same tick the branches view runs on; with nothing outstanding this loop does not use one.
const TICK: std::time::Duration = std::time::Duration::from_millis(80);

/// What the line says when the list could not be read again after something that changed it
/// or may have.
///
/// A condition, not a report: it is true of the rows on screen for as long as no reading
/// works, whatever the user presses in the meantime, and the reading that puts the list
/// right is what takes it back. So it is held by `PanesState::set_stale` and drawn beside
/// the account of what happened rather than joined to it — the panes that stopped are what
/// that account is for, and a sentence about the list neither replaces it nor belongs to
/// the same moment.
const STALE: &str = "the list could not be read again";

/// That sentence with herdr's own words on the end of it.
fn could_not_read(error: &anyhow::Error) -> String {
    format!("{STALE}: {error:#}")
}

/// And what it says where the list is the only answer there is: a removal this side could get
/// no outcome from, so the checkout may be gone and may be standing. Said once a frame,
/// because there is one list, so it is worded for however many endings nobody could read.
/// Unlike [`STALE`] this one is a report: it is about a reading that happened, in this
/// frame, and nothing later makes it untrue.
///
/// It says what was done and stops there. "A row that has gone is a checkout that has gone"
/// would be the useful sentence and it is not a sound one: the tree is built from the panes
/// herdr reports, so a repository whose last pane this removal just closed is not listed at
/// all afterwards — every one of its rows goes, none of its checkouts did — and a repository
/// `worktree.list` refused is dropped in silence. Issues #56 and #33. Read against those, a
/// decision procedure on the rows is a wrong answer given confidently, which is worse than
/// the one it replaces.
const READ_AGAIN: &str = "the list has been read again since";

/// What the picker was left wanting when it closed. The caller decides whether that means
/// switching views or exiting.
pub enum Exit {
    Closed,
    /// `None` when the cursor was not in a repository: the branches picker opens on its
    /// repository list either way.
    ShowBranches {
        repo_root: Option<String>,
        show_worktrees_without_panes: bool,
    },
}

/// Values that control one panes view without access to its event loop.
pub struct Options<'a> {
    pub initial_pane: Option<&'a str>,
    pub theme: &'a Theme,
    pub show_worktrees_without_panes: bool,
    /// Named on the prompt line the first time this view needs the file.
    pub config_complaint: Option<String>,
}

/// Run the picker to completion on the terminal the picker already holds. `run_picker` puts
/// it back on every path out, so a failure still surfaces as text rather than as a corrupted
/// screen — it is simply printed after the picker has finished rather than before.
pub fn run(
    terminal: &mut DefaultTerminal,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    removals: &mut Removals,
    pending: &mut Pending,
    options: Options<'_>,
) -> Result<Exit> {
    let (_, tree) = collect::collect_tree(herdr, git)?;
    let mut state = PanesState::new(tree, home_dir());
    state.set_show_worktrees_without_panes(options.show_worktrees_without_panes);
    if let Some(complaint) = options.config_complaint {
        state.set_message(complaint);
    }
    // Both outlive this view: a removal started before a trip through the branches view is
    // still going, and a working tree walked once does not need walking again. Only
    // `set_removing` has to be seeded, because `show_answers` never touches it — the
    // removals are not its to know about. `set_working_trees` and `set_waiting` need no
    // seeding: `show_answers` sets both every frame, and the first frame runs before the
    // first draw.
    pending.dirty.ask(state.tree());
    state.set_removing(removals.paths());
    if let Some(pane_id) = options.initial_pane {
        state.focus_pane(pane_id);
    }

    // The spinner runs on a clock rather than on redraws, so it neither speeds up while the
    // user types nor stalls while they hold a key down.
    let mut last_tick = std::time::Instant::now();
    let outcome = loop {
        let still_coming = show_answers(&mut state, pending);
        // Everything the loop is waiting on, not only the removals: `waiting` is both what
        // advances the spinner and what makes this poll instead of blocking on a key. Left
        // out, `asking gh…` draws frame zero for ever and the answer lands whenever the user
        // next happens to press something.
        let waiting = !removals.is_empty() || still_coming;
        if waiting && last_tick.elapsed() >= TICK {
            state.tick();
            last_tick = std::time::Instant::now();
        }
        let mut asked = true;
        terminal.draw(|frame| asked = render::draw(frame, &state, options.theme, Mode::Panes))?;
        if !asked {
            // The pane is too small to put the question in. Taking it back is the only
            // honest answer: the alternative is `y` armed over a box nobody ever saw.
            state.cancel_removal();
            state.set_message("this pane is too small to ask that safely".into());
        }

        drain_finished(&mut state, pending, removals, herdr, git);

        // With nothing in flight there is nothing to wake up for, so the loop blocks on the
        // key and draws no frames at all until one arrives.
        if waiting && !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        match state.handle_key(key) {
            Action::Consumed | Action::Ignored => {}
            // `r` is the only thing outside a sweep that asks about the working trees
            // again, so a reload that quietly does nothing is a reload the user reads as
            // "still dirty, then".
            Action::Reload => re_read(&mut state, pending, herdr, git, ReRead::Key),
            // A sweep's `Enter`: the same re-read, and `show_answers` puts the question up
            // once the walk has answered.
            Action::SweepReload => re_read(&mut state, pending, herdr, git, ReRead::ForSweep),
            // Deleting is housekeeping, and housekeeping comes in batches: the picker stays
            // open on the list the deletion is changing rather than closing over it — and
            // the deletion itself goes to a process of its own, so that neither the loop
            // nor the user has to wait for git to walk a working tree. See
            // `docs/adr/0014-removing-outlives-the-picker.md`.
            Action::RemoveWorktree(removal) => start_removal(
                &mut state,
                &mut pending.dirty,
                removals,
                herdr,
                git,
                &removal,
            ),
            Action::RemoveWorktrees(sweep) => start_sweep(&mut state, removals, herdr, &sweep),
            action => break action,
        }
    };

    finish(herdr, outcome, &state)
}

/// Take in every removal that has reported back since the last frame — including from before
/// the last trip to the branches view, since the removals outlive both views and the picker
/// itself — and say on the prompt line what they said.
///
/// Out of the loop for the reason `show_answers` is: this is the arm with consequences. That
/// reports share the line is held by `two_removals_reporting_in_one_frame_share_the_line`,
/// and that a withdrawal is said after them by `a_question_the_re_read_took_back_is_said_last`
/// and `a_question_taken_back_after_two_reports_is_still_said_last`.
fn drain_finished(
    state: &mut PanesState,
    pending: &mut Pending,
    removals: &mut Removals,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
) {
    // Several can report in one frame — a sweep starts them together — and each names
    // its own checkout, so they share the line rather than overwrite it. A question the
    // re-read below takes back is said on the same line, last: the reports have their
    // toasts, and the withdrawal has nothing else.
    let mut said: Vec<String> = Vec::new();
    // Whether anything reported at all, and whether any of them left nobody able to say what
    // became of the checkout.
    let mut reported = false;
    let mut unknown = false;
    while let Some(finished) = removals.finished() {
        reported = true;
        state.set_removing(removals.paths());
        match finished.outcome {
            Ok(outcome) => {
                // Nothing to say when it worked: the row leaving the list is the report,
                // and the toast has already said it to whoever was not looking.
                if let Some(message) =
                    removal::message(&finished.label, &outcome, finished.panes_closed)
                {
                    said.push(message);
                }
            }
            // Nobody knows how this one ended. `Detached::wait` is explicit that the removal
            // may well have happened, and the toast that would have said so is the child's
            // own and does not come back through here — so the list is the only answer there
            // is. The panes are gone either way, which `refusal` adds.
            Err(error) => {
                unknown = true;
                said.push(removal::refusal(
                    &format!("{error:#}"),
                    finished.panes_closed,
                ));
            }
        }
    }
    // Once, after all of them, because there is one list. Read per removal it is read N
    // times for the one answer, every reading but the last thrown away — and the line is
    // left to reconcile a reading that failed with a later one that worked, about the one
    // list. Here it also puts the order the reader needs beyond reach of a mistake: what
    // became of the list, and then the question that went with it.
    //
    // Whatever they reported, including a refusal, which changes nothing by itself: a
    // reading `start_removal` could not finish leaves rows standing for panes that have
    // stopped, that failure is a sentence and nothing more, and this is the only thing that
    // tries again on a frame nobody pressed a key for. Skipping the ones that changed
    // nothing buys a herdr round trip and gives back the bug above it.
    if reported {
        match catch_up(state, &mut pending.dirty, herdr, git) {
            Ok(withdrawn) => {
                // Only where something needs settling. After an ordinary report the rows are
                // the report, and saying they have been read would be saying nothing twice.
                if unknown {
                    said.push(READ_AGAIN.to_string());
                }
                if withdrawn {
                    said.push(WITHDRAWN.to_string());
                }
            }
            Err(error) => {
                // A question waiting on this goes back with it, the way `re_read` takes one
                // back and for the same reason: the removal that has just reported is the
                // change the box would be answered over, and the reading that would have
                // noticed is the one that failed. `replace_tree` never ran, so nothing else
                // will take it.
                //
                // Not `WITHDRAWN`, which says the list changed: nothing was replaced here,
                // and the rows are the ones that were already there. What happened is that
                // the reading failed, so this says that and what it cost — and which of the
                // two it cost, because the way back differs. A box is asked again by
                // choosing the row and pressing the key; an `Enter` that never became one
                // leaves every mark standing, so `Enter` again is the whole of it. That
                // second sentence is `re_read`'s, for the same state reached the other way.
                state.set_stale(could_not_read(&error));
                // What the failed reading cost, which is an account of this moment and not
                // a description of the list: it stays a push. The list being behind is the
                // condition beside it, and it retires when a reading works rather than when
                // a key is pressed.
                match state.cancel_removal() {
                    Cancelled::Question => said.push("the question went back".to_string()),
                    Cancelled::Enter => said.push("nothing was asked".to_string()),
                    Cancelled::Nothing => {}
                }
            }
        }
    }
    if !said.is_empty() {
        state.set_message(said.join("; "));
    }
}

/// Why the tree and the working trees are being read again.
enum ReRead {
    /// `r`. `gh` is forgotten too, since a pull request can land while the picker is up.
    Key,
    /// A sweep's `Enter`. `gh` is kept, since it only widens the sweep —
    /// `docs/adr/0011-what-may-be-swept.md`.
    ForSweep,
}

/// Read the tree and the working trees again, and say on the prompt line when that fails.
///
/// Reload means reload: whether a checkout is dirty is a fact about a working tree the user
/// has been editing since it was last asked.
fn re_read(
    state: &mut PanesState,
    pending: &mut Pending,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    why: ReRead,
) {
    match collect::collect_tree(herdr, git) {
        Ok((_, tree)) => {
            state.replace_tree(tree);
            pending.dirty.reask(state.tree());
            if matches!(why, ReRead::Key) {
                pending.settled.forget();
            }
            state.set_working_trees(pending.dirty.answers());
        }
        Err(error) => {
            // A sweep's question waiting on this goes with it — `cancel_removal` says why —
            // and the line says so, since no box is the only other sign.
            let cancelled = state.cancel_removal();
            state.set_stale(could_not_read(&error));
            if matches!(why, ReRead::ForSweep) && cancelled != Cancelled::Nothing {
                state.set_message("nothing was asked".to_string());
            }
        }
    }
}

/// Read the tree again because something certainly happened to it, and say whether a question
/// went with it.
///
/// The other half of [`re_read`], and the difference is who asked. `r` and a sweep's `Enter`
/// are the user asking, and what they get is every working tree walked again — and, for `r`
/// alone, `gh` forgotten as well. This is the picker catching up with panes it closed and
/// checkouts that may have gone, so the working trees are only brought up to the new tree:
/// `ask` walks the checkouts that have just appeared and forgets the ones that have left, and
/// a removal is no reason to doubt an answer git has already given about anything else.
///
/// `Err` is herdr's own words and only ever herdr's: the one `?` inside `collect_tree` is the
/// session, and every git call under it keeps what it can and drops the rest. They are handed
/// back rather than put on the line, because what to say about a reading that failed depends
/// on what else the frame has to say — beside an account of what happened where there is one,
/// and alone where there is not.
fn catch_up(
    state: &mut PanesState,
    dirty: &mut Dirty,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
) -> Result<bool> {
    let (_, tree) = collect::collect_tree(herdr, git)?;
    let withdrawn = state.replace_tree(tree);
    dirty.ask(state.tree());
    Ok(withdrawn)
}

/// Start every removal a sweep's `y` agreed to, in the order the box listed them.
///
/// Each that will not start is said on the prompt line, together — a refusal with no toast
/// behind it, since the child never ran — and none stops the rest: ADR 0011 has a
/// refused checkout reported on its own while the sweep carries on. No re-read afterwards:
/// a swept checkout has no panes, so nothing on screen is known wrong the way it is after
/// `start_removal` has closed some.
fn start_sweep(
    state: &mut PanesState,
    removals: &mut Removals,
    herdr: &dyn HerdrPort,
    sweep: &SweepRemoval,
) {
    let mut refused: Vec<String> = Vec::new();
    for removal in sweep.removals() {
        if let Err(message) = removals.remove(herdr, removal) {
            refused.push(message);
        }
    }
    if !refused.is_empty() {
        state.set_message(refused.join("; "));
    }
    state.set_removing(removals.paths());
}

/// Carry out a removal the user has said yes to, and put what happened on the screen.
///
/// Out of the loop for the reason `show_answers` is: `run` needs a terminal and a keyboard,
/// so nothing in it is reachable from a test, and this is the arm with consequences. What
/// deleting it looks like is a picker where `y` closes the confirmation box and does
/// nothing else — no error, no message, the row unchanged — which is a shape that reads
/// as working and is not caught by anything else here.
fn start_removal(
    state: &mut PanesState,
    dirty: &mut Dirty,
    removals: &mut Removals,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    removal: &Removal,
) {
    let refused = removals.remove(herdr, removal).err();
    // The row says what is happening to it; nothing to add here. Unconditionally, as
    // `start_sweep` does: a removal that never started pushed nothing, so this is what it
    // already was.
    state.set_removing(removals.paths());

    // Wherever the removal named panes, and whatever became of it. On the way through, every
    // one of them closed; on the way out, either everything up to the one that refused, or —
    // where it was the start that failed — every one of them. And a `pane_close` that failed
    // is no evidence the pane survived: the adapter answers `Ok` for a pane herdr no longer
    // knows about, so an `Err` is a call this side cannot account for. Every way round, a
    // row standing for a pane that has stopped is one the cursor lands on, and `Enter` on it
    // takes the whole picker down: `pane_focus` fails, and the `?` in `perform` carries that
    // out of `run` with only the log to say why. An empty checkout's removal changes nothing
    // on screen yet and leaves the cursor where it was, which is where the next thing to
    // tidy up usually is.
    //
    // The `bool` is dropped rather than said: a question cannot be up here, because
    // `handle_key` takes `pending_removal` in the act of returning the action this is
    // carrying out, and `Shift-D` is ignored inside a sweep.
    if !removal.panes().is_empty() {
        if let Err(error) = catch_up(state, dirty, herdr, git) {
            // Not fatal, but not silent either: rows for panes that have certainly stopped
            // are on screen until a reading works, which is exactly a condition's shape.
            state.set_stale(could_not_read(&error));
        }
    }
    // The account of what happened. It no longer shares the line with a sentence about the
    // list, so it is never the half that gets written over: on this arm it may be a refusal
    // that stopped nothing at all, and that is the half no other thing on screen will say.
    if let Some(message) = refused {
        state.set_message(message);
    }
}

/// Tell the state what the walk and `gh` have said since the last frame, put a sweep's
/// question up once its walk has answered, and say whether either has more to come.
///
/// Split out of the loop because everything the loop does is otherwise untestable — it needs
/// a terminal and a keyboard — and this is the part with consequences. `set_working_trees` in
/// particular has to run every frame and not only when a marker moved: a clean answer moves
/// none, and it is exactly the answer that turns a refusal into an offer. Left out, every
/// checkout with panes in it answers "still reading that working tree" for the life of the
/// picker, and nothing on screen or in the suite says why.
fn show_answers(state: &mut PanesState, pending: &mut Pending) -> bool {
    let Pending { dirty, settled } = pending;
    dirty.drain();
    // Unconditionally, and not only when a marker moved: `PanesState` keeps every answer and
    // decides for itself what is worth redrawing. What is *known* about a working tree and
    // what is *drawn* about it are different questions, and a removal turns on the first.
    state.set_working_trees(dirty.answers());
    let reading = dirty.is_waiting();
    state.set_waiting(reading);

    // The heavier of the two `gh` calls, so it is asked when a sweep is entered rather than
    // when the picker opens — ADR 0011. Asked from here rather than from the key that
    // enters the sweep because `ask` is the same question every time and answers it once:
    // a `Tab` away and back, or a sweep left and re-entered, costs a map lookup rather than
    // another round of `gh`.
    if state.is_sweeping() {
        // On the frame after `Shift-S`, and on no other: a `gh` that could not answer when
        // the sweep was last entered is asked again, and one that answered is not.
        if state.sweep_entered() {
            settled.forget_failures();
        }
        settled.ask(state.tree());
    }
    settled.drain();
    let answered = settled.answers(state.tree());
    let trouble = settled.trouble(state.tree());
    // Ignored outside a sweep, which is where the answers would have nowhere to be shown.
    let asking = settled.is_waiting(state.tree());
    state.set_settled(answered, trouble, asking);
    state.confirm_sweep_if_settled();

    // Both, because the loop's clock has to run while either is out — and because a sweep
    // entered on a slow network is exactly when a frozen spinner reads as a finished answer.
    reading || asking
}

fn perform(
    herdr: &dyn HerdrPort,
    action: Action,
    show_worktrees_without_panes: bool,
) -> Result<Exit> {
    match action {
        Action::Quit => Ok(Exit::Closed),
        // Focus, then exit. herdr tears the overlay down once this process ends, and the
        // focus set just before that is what the user is left looking at.
        Action::Jump(pane_id) => {
            herdr.pane_focus(&pane_id)?;
            Ok(Exit::Closed)
        }
        Action::OpenWorktree {
            repo_root,
            checkout_path,
        } => {
            herdr.worktree_open(&WorktreeOpen {
                cwd: repo_root,
                path: Some(checkout_path),
                branch: None,
                focus: true,
            })?;
            Ok(Exit::Closed)
        }
        Action::NewPane {
            checkout_path,
            beside_pane_id,
        } => {
            herdr.pane_split(&PaneSplit {
                target_pane_id: beside_pane_id,
                direction: SplitDirection::Right,
                cwd: Some(checkout_path),
                focus: true,
            })?;
            Ok(Exit::Closed)
        }
        Action::ShowBranches { repo_root } => Ok(Exit::ShowBranches {
            repo_root,
            show_worktrees_without_panes,
        }),
        // Handled inside the loop, which is why the picker is still up after one.
        Action::Consumed
        | Action::Ignored
        | Action::Reload
        | Action::SweepReload
        | Action::RemoveWorktree { .. }
        | Action::RemoveWorktrees { .. } => Ok(Exit::Closed),
    }
}

/// Carry view state into the exit that the parent loop keeps across a view switch.
fn finish(herdr: &dyn HerdrPort, action: Action, state: &PanesState) -> Result<Exit> {
    perform(herdr, action, state.shows_worktrees_without_panes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fakes::{
        until, Lost, LostFirst, Recorder, Refuses, RefusesFirst, Reports, Started, Unwaited,
    };
    use crate::app::settled::Settled;
    use crate::domain::model::{CheckoutPath, RepoKey};
    use crate::port::{
        AgentStatus, GitRef, RefKind, RefWalk, RemovalOutcome, Slug, Snapshot, Track, Workspace,
        WorkspaceWorktree, Worktree, WorktreeList, WorktreeSource,
    };
    use crate::ui::state::PanesState;
    use anyhow::Result;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    /// Answers every working tree at once and calls it clean, which is what makes the
    /// difference between "asked" and "answered" observable in one drain.
    /// A git that gives the same answer for every working tree, or the same failure.
    /// All three matter: clean is what lets a removal through, and dirty and unreadable are
    /// what the two refusals protecting a working agent are made of.
    struct Answers(Option<bool>);

    impl GitPort for Answers {
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            self.0
                .ok_or_else(|| anyhow::anyhow!("fatal: not a git repository"))
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!()
        }
        fn github_slug(&self, _repo_root: &str) -> Result<Option<crate::port::Slug>> {
            unreachable!()
        }
        fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
            unreachable!()
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!()
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!()
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!()
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!()
        }
        fn delete_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!()
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!()
        }
    }

    impl crate::port::GhPort for Answers {
        fn pull_requests(&self, _slug: &crate::port::Slug) -> Vec<crate::port::PullRequest> {
            unreachable!("the panes view does not decorate")
        }
        fn settled_pull_requests(
            &self,
            _slug: &crate::port::Slug,
        ) -> std::result::Result<crate::port::SettledPullRequests, String> {
            unreachable!("these tests never enter a sweep")
        }
    }

    /// A git that never gets round to naming the repository, so a sweep's `gh` question
    /// stays outstanding for exactly as long as the test wants it to. Parking rather than
    /// sleeping, so the wait is decided by the test rather than by a duration.
    struct Never;

    impl GitPort for Never {
        fn github_slug(&self, _repo_root: &str) -> Result<Option<crate::port::Slug>> {
            std::thread::park();
            unreachable!("nothing unparks this")
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("only github_slug is asked of this port")
        }
        fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
            unreachable!("only github_slug is asked of this port")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("only github_slug is asked of this port")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn delete_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            unreachable!("only github_slug is asked of this port")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("only github_slug is asked of this port")
        }
    }

    impl crate::port::GhPort for Never {
        fn pull_requests(&self, _slug: &crate::port::Slug) -> Vec<crate::port::PullRequest> {
            unreachable!("the sweep does not decorate")
        }
        fn settled_pull_requests(
            &self,
            _slug: &crate::port::Slug,
        ) -> std::result::Result<crate::port::SettledPullRequests, String> {
            unreachable!("git never names a repository to ask about")
        }
    }

    /// A `gh` that answers, and remembers what it was asked and how often. Everything the
    /// loop does with the sweep's half of the question goes through this.
    #[derive(Default)]
    struct Answering {
        asked: std::sync::Mutex<Vec<String>>,
        /// What `gh` says. `Err` is a `gh` that refused, which is a sentence for the prompt
        /// line rather than an empty answer — see ADR 0011.
        answer: Option<std::result::Result<crate::port::SettledPullRequests, String>>,
    }

    impl Answering {
        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl GitPort for Answering {
        fn github_slug(&self, repo_root: &str) -> Result<Option<crate::port::Slug>> {
            self.asked.lock().unwrap().push(repo_root.to_string());
            Ok(crate::port::Slug::owner_repo("me", "app"))
        }
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            Ok(false)
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("the loop asks this port two things")
        }
        fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
            unreachable!("the loop asks this port two things")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("the loop asks this port two things")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("the loop asks this port two things")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("the loop asks this port two things")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("the loop asks this port two things")
        }
        fn delete_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("the loop asks this port two things")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("the loop asks this port two things")
        }
    }

    impl crate::port::GhPort for Answering {
        fn pull_requests(&self, _slug: &crate::port::Slug) -> Vec<crate::port::PullRequest> {
            unreachable!("the panes view does not decorate")
        }
        fn settled_pull_requests(
            &self,
            _slug: &crate::port::Slug,
        ) -> std::result::Result<crate::port::SettledPullRequests, String> {
            self.answer
                .clone()
                .unwrap_or_else(|| Ok(crate::port::SettledPullRequests::All(Vec::new())))
        }
    }

    /// A `gh` that refuses the first time it is asked and answers every time after: the
    /// network coming back, or a token renewed.
    #[derive(Default)]
    struct Recovering {
        asked: std::sync::Mutex<usize>,
    }

    impl Recovering {
        fn asked(&self) -> usize {
            *self.asked.lock().unwrap()
        }
    }

    impl GitPort for Recovering {
        fn github_slug(&self, _repo_root: &str) -> Result<Option<crate::port::Slug>> {
            Ok(crate::port::Slug::owner_repo("me", "app"))
        }
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            Ok(false)
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("the loop asks this port two things")
        }
        fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
            unreachable!("the loop asks this port two things")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("the loop asks this port two things")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("the loop asks this port two things")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("the loop asks this port two things")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("the loop asks this port two things")
        }
        fn delete_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("the loop asks this port two things")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("the loop asks this port two things")
        }
    }

    impl crate::port::GhPort for Recovering {
        fn pull_requests(&self, _slug: &crate::port::Slug) -> Vec<crate::port::PullRequest> {
            unreachable!("the panes view does not decorate")
        }
        fn settled_pull_requests(
            &self,
            _slug: &crate::port::Slug,
        ) -> std::result::Result<crate::port::SettledPullRequests, String> {
            let mut asked = self.asked.lock().unwrap();
            *asked += 1;
            if *asked == 1 {
                return Err("gh refused the question this asked: could not connect".into());
            }
            Ok(crate::port::SettledPullRequests::All(Vec::new()))
        }
    }

    fn press(state: &mut PanesState, key: char) {
        state.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char(key),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    const FEAT_LOGIN: &str = "/wt/feat-login";
    const FIX_CRASH: &str = "/wt/fix-crash";

    /// A herdr that can describe its session and a git that answers for it — everything
    /// `collect_tree` reads. One repository: its own checkout with a pane in it, and two
    /// finished worktrees with none. The second reading can differ from the first in one of
    /// the two ways issue #25 is about.
    struct Session {
        /// A pane opened in `feat/login` between the first reading and the second.
        pane_opens: bool,
        /// A file written into `feat/login` between the first walk and the second.
        file_written: bool,
        readings: Mutex<usize>,
        walks: Mutex<BTreeMap<String, usize>>,
    }

    impl Session {
        fn new(pane_opens: bool, file_written: bool) -> Self {
            Self {
                pane_opens,
                file_written,
                readings: Mutex::new(0),
                walks: Mutex::new(BTreeMap::new()),
            }
        }

        fn pane_is_open(&self) -> bool {
            self.pane_opens && *self.readings.lock().unwrap() >= 2
        }

        /// How often the walk asked about this checkout.
        fn walks_of(&self, checkout_path: &str) -> usize {
            self.walks
                .lock()
                .unwrap()
                .get(checkout_path)
                .copied()
                .unwrap_or(0)
        }
    }

    fn snapshot_pane(id: &str, cwd: &str) -> crate::port::Pane {
        let workspace = id.split(':').next().unwrap().to_string();
        crate::port::Pane {
            pane_id: id.into(),
            tab_id: format!("{workspace}:t1"),
            workspace_id: workspace,
            terminal_id: String::new(),
            cwd: Some(cwd.into()),
            foreground_cwd: None,
            focused: false,
            agent: Some("codex".into()),
            agent_status: AgentStatus::Idle,
            title: None,
            terminal_title_stripped: None,
            label: None,
        }
    }

    fn workspace(id: &str, checkout_path: &str, linked: bool) -> Workspace {
        Workspace {
            workspace_id: id.into(),
            label: String::new(),
            number: 0,
            focused: false,
            active_tab_id: None,
            agent_status: AgentStatus::Unknown,
            worktree: Some(WorkspaceWorktree {
                repo_key: "/src/app/.git".into(),
                repo_name: "app".into(),
                repo_root: "/src/app".into(),
                checkout_path: checkout_path.into(),
                is_linked_worktree: linked,
            }),
        }
    }

    fn listed(branch: &str, path: &str, linked: bool, open_in: Option<&str>) -> Worktree {
        Worktree {
            branch: Some(branch.into()),
            path: path.into(),
            label: String::new(),
            is_bare: false,
            is_detached: false,
            is_linked_worktree: linked,
            is_prunable: false,
            open_workspace_id: open_in.map(str::to_string),
        }
    }

    fn gone(branch: &str, path: &str) -> GitRef {
        GitRef {
            name: branch.into(),
            kind: RefKind::Local,
            committed_at: None,
            subject: None,
            upstream: Some(format!("origin/{branch}")),
            track: Some(Track::Gone),
            worktree_path: Some(path.into()),
        }
    }

    impl HerdrPort for Session {
        fn snapshot(&self) -> Result<Snapshot> {
            *self.readings.lock().unwrap() += 1;
            let mut workspaces = vec![workspace("w1", "/src/app", false)];
            let mut panes = vec![snapshot_pane("w1:p1", "/src/app")];
            if self.pane_is_open() {
                workspaces.push(workspace("w2", FEAT_LOGIN, true));
                panes.push(snapshot_pane("w2:p1", FEAT_LOGIN));
            }
            Ok(Snapshot {
                workspaces,
                panes,
                ..Snapshot::default()
            })
        }
        fn worktree_list(&self, _cwd: &str) -> Result<WorktreeList> {
            Ok(WorktreeList {
                source: WorktreeSource {
                    repo_key: "/src/app/.git".into(),
                    repo_name: "app".into(),
                    repo_root: "/src/app".into(),
                    source_checkout_path: "/src/app".into(),
                    source_workspace_id: None,
                },
                worktrees: vec![
                    listed("main", "/src/app", false, Some("w1")),
                    listed(
                        "feat/login",
                        FEAT_LOGIN,
                        true,
                        self.pane_is_open().then_some("w2"),
                    ),
                    listed("fix/crash", FIX_CRASH, true, None),
                ],
            })
        }
        fn worktree_create(
            &self,
            _req: &crate::port::WorktreeCreate,
        ) -> Result<crate::port::WorktreeOpened> {
            unreachable!("a re-read asks this port for the session and the worktrees")
        }
        fn worktree_open(&self, _req: &WorktreeOpen) -> Result<crate::port::WorktreeOpened> {
            unreachable!("a re-read asks this port for the session and the worktrees")
        }
        fn pane_focus(&self, _pane_id: &str) -> Result<()> {
            unreachable!("a re-read asks this port for the session and the worktrees")
        }
        fn pane_split(&self, _req: &PaneSplit) -> Result<crate::port::Pane> {
            unreachable!("a re-read asks this port for the session and the worktrees")
        }
        fn pane_move(
            &self,
            _pane: &str,
            _dest: &crate::port::PaneDestination,
            _focus: bool,
        ) -> Result<()> {
            unreachable!("a re-read asks this port for the session and the worktrees")
        }
        fn pane_close(&self, _pane_id: &str) -> Result<()> {
            unreachable!("a swept checkout has no panes to close")
        }
        fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
            unreachable!("a re-read asks this port for the session and the worktrees")
        }
        fn tab_focus(&self, _tab_id: &str) -> Result<()> {
            unreachable!("a re-read asks this port for the session and the worktrees")
        }
        fn plugin_pane_open(
            &self,
            _req: &crate::port::PluginPaneOpen,
        ) -> Result<Option<crate::port::OpenRefusal>> {
            unreachable!("a re-read asks this port for the session and the worktrees")
        }
        fn notify(&self, _notification: &crate::port::Notification) -> Result<()> {
            unreachable!("a re-read asks this port for the session and the worktrees")
        }
    }

    impl GitPort for Session {
        fn is_dirty(&self, checkout_path: &str) -> Result<bool> {
            let mut walks = self.walks.lock().unwrap();
            let walked = walks.entry(checkout_path.to_string()).or_insert(0);
            *walked += 1;
            Ok(self.file_written && checkout_path == FEAT_LOGIN && *walked >= 2)
        }
        fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
            Ok(Slug::owner_repo("me", "app"))
        }
        fn local_refs(&self, _repo_root: &str) -> Result<RefWalk> {
            Ok(RefWalk::of(vec![
                gone("feat/login", FEAT_LOGIN),
                gone("fix/crash", FIX_CRASH),
            ]))
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("every pane is in a workspace herdr knows the checkout of")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("a re-read asks this port three things")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("a re-read asks this port three things")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("a re-read asks this port three things")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("a re-read asks this port three things")
        }
        fn delete_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("a re-read asks this port three things")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("a re-read asks this port three things")
        }
    }

    impl crate::port::GhPort for Session {
        fn pull_requests(&self, _slug: &Slug) -> Vec<crate::port::PullRequest> {
            unreachable!("the panes view does not decorate")
        }
        fn settled_pull_requests(
            &self,
            _slug: &Slug,
        ) -> std::result::Result<crate::port::SettledPullRequests, String> {
            Ok(crate::port::SettledPullRequests::All(Vec::new()))
        }
    }

    /// The two panes [`Closing`] starts with, in the order a removal names them.
    const FIRST_PANE: &str = "w2:p1";
    const SECOND_PANE: &str = "w2:p2";

    /// A herdr that really closes panes and then describes a session without them, and a git
    /// that answers for it. One repository: its own checkout, and `feat/login` with two panes
    /// in it.
    ///
    /// `Recorder` cannot reach this ground. It refuses to describe itself at all, which is
    /// the other half of the same question — what the list says once some of the panes have
    /// gone is only answerable by a herdr that answers.
    struct Closing {
        /// The one pane it will not close. Everything the removal names ahead of that does
        /// close, which is what makes a close that stopped partway reachable at all.
        refuses: &'static str,
        open: Mutex<Vec<String>>,
        /// Whether the checkout goes when the last of its panes does — the ordinary ending,
        /// where the removal went through between one reading and the next.
        removed_with_its_panes: bool,
    }

    impl Closing {
        fn refusing(pane_id: &'static str) -> Self {
            Self {
                refuses: pane_id,
                open: Mutex::new(vec![FIRST_PANE.to_string(), SECOND_PANE.to_string()]),
                removed_with_its_panes: false,
            }
        }

        /// Closes whatever it is asked to, and describes a session the checkout has left.
        fn where_the_checkout_goes_too() -> Self {
            Self {
                refuses: "",
                removed_with_its_panes: true,
                ..Self::refusing("")
            }
        }

        /// Whether `feat/login` is still there, which is what the two readings differ by.
        fn listed(&self) -> bool {
            !self.removed_with_its_panes || !self.open.lock().unwrap().is_empty()
        }
    }

    impl HerdrPort for Closing {
        fn pane_close(&self, pane_id: &str) -> Result<()> {
            if pane_id == self.refuses {
                return Err(anyhow::anyhow!(
                    "herdr rejected pane.close: no such pane (not_found)"
                ));
            }
            self.open.lock().unwrap().retain(|open| open != pane_id);
            Ok(())
        }
        fn snapshot(&self) -> Result<Snapshot> {
            let mut workspaces = vec![workspace("w1", "/src/app", false)];
            // The repository is only in the tree at all while a pane of it is: `collect`
            // probes from pane placements. The checkout that goes here takes the last of its
            // own panes with it, so this keeps one in the main checkout — otherwise the whole
            // repository leaves with it and there is nothing left to be forgotten.
            let mut panes = match self.removed_with_its_panes {
                true => vec![snapshot_pane("w1:p9", "/src/app")],
                false => Vec::new(),
            };
            if self.listed() {
                workspaces.push(workspace("w2", FEAT_LOGIN, true));
            }
            panes.extend(
                self.open
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|pane_id| snapshot_pane(pane_id, FEAT_LOGIN)),
            );
            Ok(Snapshot {
                workspaces,
                panes,
                ..Snapshot::default()
            })
        }
        fn worktree_list(&self, _cwd: &str) -> Result<WorktreeList> {
            let mut worktrees = vec![listed("main", "/src/app", false, Some("w1"))];
            if self.listed() {
                worktrees.push(listed("feat/login", FEAT_LOGIN, true, Some("w2")));
            }
            Ok(WorktreeList {
                source: WorktreeSource {
                    repo_key: "/src/app/.git".into(),
                    repo_name: "app".into(),
                    repo_root: "/src/app".into(),
                    source_checkout_path: "/src/app".into(),
                    source_workspace_id: None,
                },
                worktrees,
            })
        }
        fn worktree_create(
            &self,
            _req: &crate::port::WorktreeCreate,
        ) -> Result<crate::port::WorktreeOpened> {
            unreachable!("this port closes panes and describes what is left")
        }
        fn worktree_open(&self, _req: &WorktreeOpen) -> Result<crate::port::WorktreeOpened> {
            unreachable!("this port closes panes and describes what is left")
        }
        fn pane_focus(&self, _pane_id: &str) -> Result<()> {
            unreachable!("this port closes panes and describes what is left")
        }
        fn pane_split(&self, _req: &PaneSplit) -> Result<crate::port::Pane> {
            unreachable!("this port closes panes and describes what is left")
        }
        fn pane_move(
            &self,
            _pane: &str,
            _dest: &crate::port::PaneDestination,
            _focus: bool,
        ) -> Result<()> {
            unreachable!("this port closes panes and describes what is left")
        }
        fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
            unreachable!("this port closes panes and describes what is left")
        }
        fn tab_focus(&self, _tab_id: &str) -> Result<()> {
            unreachable!("this port closes panes and describes what is left")
        }
        fn plugin_pane_open(
            &self,
            _req: &crate::port::PluginPaneOpen,
        ) -> Result<Option<crate::port::OpenRefusal>> {
            unreachable!("this port closes panes and describes what is left")
        }
        fn notify(&self, _notification: &crate::port::Notification) -> Result<()> {
            unreachable!("this port closes panes and describes what is left")
        }
    }

    impl GitPort for Closing {
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            Ok(false)
        }
        fn local_refs(&self, _repo_root: &str) -> Result<RefWalk> {
            Ok(RefWalk::of(vec![gone("feat/login", FEAT_LOGIN)]))
        }
        fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
            Ok(Slug::owner_repo("me", "app"))
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("every pane is in a workspace herdr knows the checkout of")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("a reading asks this port two things")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("a reading asks this port two things")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("a reading asks this port two things")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("a reading asks this port two things")
        }
        fn delete_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("a reading asks this port two things")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("a reading asks this port two things")
        }
    }

    /// A herdr whose refusal has a cause under it, which is the shape the socket adapter
    /// makes: `with_context` over the io error that actually happened. Every other fake here
    /// refuses with one flat sentence, so the alternate form that prints the chain is
    /// invisible to them.
    struct Chained;

    impl HerdrPort for Chained {
        fn pane_close(&self, _pane_id: &str) -> Result<()> {
            Ok(())
        }
        fn snapshot(&self) -> Result<Snapshot> {
            Err(anyhow::anyhow!("broken pipe").context("sending session.snapshot to herdr"))
        }
        fn worktree_list(&self, _cwd: &str) -> Result<WorktreeList> {
            unreachable!("the session is asked for first, and it refuses")
        }
        fn worktree_create(
            &self,
            _req: &crate::port::WorktreeCreate,
        ) -> Result<crate::port::WorktreeOpened> {
            unreachable!("this port closes panes and then refuses to describe itself")
        }
        fn worktree_open(&self, _req: &WorktreeOpen) -> Result<crate::port::WorktreeOpened> {
            unreachable!("this port closes panes and then refuses to describe itself")
        }
        fn pane_focus(&self, _pane_id: &str) -> Result<()> {
            unreachable!("this port closes panes and then refuses to describe itself")
        }
        fn pane_split(&self, _req: &PaneSplit) -> Result<crate::port::Pane> {
            unreachable!("this port closes panes and then refuses to describe itself")
        }
        fn pane_move(
            &self,
            _pane: &str,
            _dest: &crate::port::PaneDestination,
            _focus: bool,
        ) -> Result<()> {
            unreachable!("this port closes panes and then refuses to describe itself")
        }
        fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
            unreachable!("this port closes panes and then refuses to describe itself")
        }
        fn tab_focus(&self, _tab_id: &str) -> Result<()> {
            unreachable!("this port closes panes and then refuses to describe itself")
        }
        fn plugin_pane_open(
            &self,
            _req: &crate::port::PluginPaneOpen,
        ) -> Result<Option<crate::port::OpenRefusal>> {
            unreachable!("this port closes panes and then refuses to describe itself")
        }
        fn notify(&self, _notification: &crate::port::Notification) -> Result<()> {
            unreachable!("this port closes panes and then refuses to describe itself")
        }
    }

    /// Open the picker on `session` the way `run` does, enter a sweep with both finished
    /// worktrees marked, and press `Enter`.
    fn enter_pressed(session: &Arc<Session>) -> (PanesState, Pending) {
        let (_, tree) = collect::collect_tree(&**session, &**session).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut pending = Pending {
            dirty: Dirty::new(session.clone()),
            settled: Settled::new(session.clone(), session.clone()),
        };
        pending.dirty.ask(state.tree());
        press(&mut state, 'S');
        settle(&mut state, &mut pending);
        let chosen: Vec<String> = state
            .chosen()
            .into_iter()
            .map(|(_, path)| path.as_str().to_string())
            .collect();
        assert_eq!(
            chosen,
            [FEAT_LOGIN, FIX_CRASH],
            "both are gone, clean, and have nothing running in them"
        );
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::SweepReload);
        (state, pending)
    }

    /// `enter_pressed`, then the re-read it asked for and the walk that starts, driven to
    /// the frame that puts the box up.
    fn sweep_asked(session: &Arc<Session>) -> (PanesState, Pending) {
        let (mut state, mut pending) = enter_pressed(session);
        re_read(
            &mut state,
            &mut pending,
            &**session,
            &**session,
            ReRead::ForSweep,
        );
        settle(&mut state, &mut pending);
        (state, pending)
    }

    /// What the sweep's box lists, by path.
    fn box_paths(state: &PanesState) -> Vec<String> {
        state
            .pending_sweep()
            .map(|sweep| {
                sweep
                    .removals()
                    .iter()
                    .map(|removal| removal.checkout_path().as_str().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_pane_opened_after_the_sweep_began_takes_that_checkout_off_the_list_before_anything_is_removed(
    ) {
        let session = Arc::new(Session::new(true, false));
        let (mut state, _pending) = sweep_asked(&session);
        assert_eq!(
            box_paths(&state),
            [FIX_CRASH],
            "the checkout with the pane is off the list"
        );
        assert_eq!(state.message(), Some("no longer marked: feat/login"));

        // And what `y` removes is what the box listed.
        let recorder = Recorder::default();
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);
        let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        start_sweep(&mut state, &mut removals, &recorder, &sweep);
        assert_eq!(
            recorder.did(),
            ["start /wt/fix-crash after 0 deleting the branch"]
        );
    }

    #[test]
    fn a_file_written_after_the_sweep_began_does_the_same() {
        let session = Arc::new(Session::new(false, true));
        let (state, _pending) = sweep_asked(&session);
        assert_eq!(
            session.walks_of(FEAT_LOGIN),
            2,
            "the working tree was walked again"
        );
        assert_eq!(box_paths(&state), [FIX_CRASH]);
        assert_eq!(state.message(), Some("no longer marked: feat/login"));
    }

    #[test]
    fn one_refusal_to_start_does_not_stop_the_rest_of_the_sweep() {
        let session = Arc::new(Session::new(false, false));
        let (mut state, _pending) = sweep_asked(&session);
        assert_eq!(box_paths(&state), [FEAT_LOGIN, FIX_CRASH]);

        let recorder = Recorder::default();
        let port = RefusesFirst::new(&recorder);
        let mut removals = Removals::new(&port);
        let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        start_sweep(&mut state, &mut removals, &recorder, &sweep);

        assert_eq!(
            recorder.did(),
            ["start /wt/fix-crash after 0 deleting the branch"],
            "the second started although the first would not"
        );
        assert_eq!(
            state.message(),
            Some(
                "could not start removing feat/login: could not spawn: no such file or \
                 directory"
            )
        );
        assert_eq!(removals.paths(), [CheckoutPath::for_test(FIX_CRASH)]);
        assert!(
            state.rows().iter().any(|row| row.is_removing),
            "and the row that is going says so"
        );
    }

    #[test]
    fn a_re_read_that_fails_asks_nothing_over_the_old_facts() {
        // `Enter` asked for the re-read so that the box is about the disk as it is; with
        // herdr not answering, a box would be about the disk as it was.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = enter_pressed(&session);
        re_read(
            &mut state,
            &mut pending,
            &Recorder::default(),
            &*session,
            ReRead::ForSweep,
        );
        settle(&mut state, &mut pending);

        assert!(state.pending_sweep().is_none());
        assert_eq!(state.message(), Some("nothing was asked"));
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Char('y'))),
            Action::Ignored,
            "and `y` answers nothing"
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Action::SweepReload,
            "asking again asks for the re-read again"
        );
    }

    #[test]
    fn enter_keeps_what_gh_said_and_r_does_not() {
        // `gh` only widens the sweep (ADR 0011), so the re-read `Enter` asks for keeps its
        // answers; `r` reads everything again, `gh` included.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = enter_pressed(&session);
        assert_eq!(
            pending.settled.answers(state.tree()).len(),
            1,
            "the sweep asked gh"
        );

        re_read(
            &mut state,
            &mut pending,
            &*session,
            &*session,
            ReRead::ForSweep,
        );
        assert_eq!(
            pending.settled.answers(state.tree()).len(),
            1,
            "and `Enter` kept it"
        );

        re_read(&mut state, &mut pending, &*session, &*session, ReRead::Key);
        assert!(
            pending.settled.answers(state.tree()).is_empty(),
            "`r` forgets it"
        );
    }

    /// Drive the loop's per-frame step until nothing is outstanding, the way `run` does.
    fn settle(state: &mut PanesState, pending: &mut Pending) {
        until("the frame never stopped waiting", || {
            !show_answers(state, pending)
        });
    }

    /// A walk in progress and a sweep whose `gh` question will not come back.
    fn pending_on_gh(dirty: Dirty) -> Pending {
        let port = std::sync::Arc::new(Never);
        Pending {
            dirty,
            settled: crate::app::settled::Settled::new(port.clone(), port),
        }
    }

    /// A walk in progress and a sweep nobody has entered. `show_answers` asks `gh` nothing
    /// until one is, which is what lets these ports say `unreachable!()` and mean it.
    fn pending(dirty: Dirty) -> Pending {
        let port = std::sync::Arc::new(Answers(Some(false)));
        Pending {
            dirty,
            settled: crate::app::settled::Settled::new(port.clone(), port),
        }
    }

    fn no_pane_tree() -> crate::domain::model::Tree {
        let mut tree = one_pane_tree();
        tree.repos[0].worktrees[0].panes.clear();
        tree
    }

    fn one_pane_tree() -> crate::domain::model::Tree {
        use crate::domain::model::{PaneNode, Refs, RepoNode, Tree, WorktreeNode};
        Tree {
            repos: vec![RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
                refs: Refs::Read,
                worktrees: vec![WorktreeNode {
                    branch: Some("feat/login".into()),
                    checkout_path: "/wt/feat-login".into(),
                    is_primary: false,
                    open_workspace_id: Some("w2".into()),
                    track: None,
                    panes: vec![PaneNode {
                        pane_id: "w2:p1".into(),
                        workspace_id: "w2".into(),
                        tab_id: "w2:t1".into(),
                        display_name: Some("codex".into()),
                        agent_status: crate::port::AgentStatus::Idle,
                        focused: false,
                    }],
                }],
            }],
            ungrouped: Vec::new(),
        }
    }

    #[test]
    fn the_branches_exit_carries_the_runtime_no_pane_toggle() {
        let mut state = PanesState::new(no_pane_tree(), None);
        state.handle_key(key(KeyCode::Char('p')));
        let action = state.handle_key(key(KeyCode::Tab));

        match finish(&Recorder::default(), action, &state).unwrap() {
            Exit::ShowBranches {
                show_worktrees_without_panes,
                ..
            } => assert!(!show_worktrees_without_panes),
            Exit::Closed => panic!("Tab must switch to the branches view"),
        }
    }

    #[test]
    fn the_loop_hands_on_a_dirty_answer_too_or_nothing_is_ever_protected() {
        // The negative twin of the test below, and the one with teeth. A `Clean` answer alone
        // is what clears "still reading"; the dirty answer is what the refusal is made of.
        // Hand on the first without the second and `Shift-D` walks straight into the
        // confirmation box for a checkout full of working agents. `y` then closes every one
        // of their panes before git refuses to remove the checkout — so git saves the work
        // and nothing saves the agents.
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(true))));
        dirty.ask(state.tree());

        let mut pending = pending(dirty);
        until(
            "the walk never answered for the only checkout there is",
            || !show_answers(&mut state, &mut pending),
        );

        state.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('D'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(
            state.pending_removal().is_none(),
            "a checkout holding uncommitted work is not offered"
        );
        assert_eq!(
            state.message(),
            Some("that checkout is holding work nobody has committed"),
            "and it says which of the refusals this is"
        );
    }

    /// The checkout the one-pane tree describes, ready to be removed.
    fn only_removal(state: &PanesState) -> Removal {
        let repo = &state.tree().repos[0];
        Removal::of(&repo.repo_root, &repo.worktrees[0])
    }

    /// The same, where the tree has more than one checkout in it and the panes are in the
    /// second: what [`Closing`] describes.
    fn removal_of(state: &PanesState, checkout_path: &str) -> Removal {
        let repo = &state.tree().repos[0];
        let worktree = repo
            .worktrees
            .iter()
            .find(|worktree| worktree.checkout_path == checkout_path)
            .expect("the tree has that checkout");
        Removal::of(&repo.repo_root, worktree)
    }

    /// The panes the list is drawing, in order. A pane that has been closed and is still on
    /// screen is a row the cursor stops on and `Enter` acts on.
    fn pane_rows(state: &PanesState) -> Vec<String> {
        state
            .rows()
            .iter()
            .filter(|row| matches!(row.reference, crate::domain::rows::RowRef::Pane(..)))
            .map(|row| row.meta.clone())
            .collect()
    }

    /// The one-pane tree with a second pane in the same checkout, so that a close can stop
    /// between them.
    fn two_pane_tree() -> crate::domain::model::Tree {
        let mut tree = one_pane_tree();
        let panes = &mut tree.repos[0].worktrees[0].panes;
        let mut second = panes[0].clone();
        second.pane_id = SECOND_PANE.into();
        panes.push(second);
        tree
    }

    #[test]
    fn gh_is_asked_when_a_sweep_is_entered_and_not_before() {
        // The heavier of the two `gh` calls, so ADR 0011 defers it: most sessions never
        // sweep, and one that does should not pay for it on every picker open. Every line
        // between `show_answers` and the screen could be deleted with the whole gate green
        // — including the call that hands the answers over at all.
        let port = std::sync::Arc::new(Answering::default());
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut pending = Pending {
            dirty: Dirty::new(port.clone()),
            settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
        };
        pending.dirty.ask(state.tree());
        settle(&mut state, &mut pending);
        assert!(
            port.asked().is_empty(),
            "the picker opened and nobody swept"
        );

        press(&mut state, 'S');
        settle(&mut state, &mut pending);
        assert_eq!(
            port.asked(),
            ["/src/app"],
            "asked once, about the repository root"
        );

        // And a second frame, and leaving and coming back, ask nothing more: a merged pull
        // request does not become unmerged.
        settle(&mut state, &mut pending);
        press(&mut state, 'q');
        press(&mut state, 'S');
        settle(&mut state, &mut pending);
        assert_eq!(port.asked().len(), 1);
    }

    #[test]
    fn what_gh_said_reaches_the_rows_and_what_went_wrong_reaches_the_prompt() {
        let port = std::sync::Arc::new(Answering {
            answer: Some(Err(
                "gh refused the question this asked: no auth".to_string()
            )),
            ..Answering::default()
        });
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut pending = Pending {
            dirty: Dirty::new(port.clone()),
            settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
        };
        pending.dirty.ask(state.tree());

        press(&mut state, 'S');
        settle(&mut state, &mut pending);

        assert_eq!(
            state.sweep_trouble(),
            Some("me/app: gh refused the question this asked: no auth"),
            "the rows can say a checkout could not be judged; only this says which and why"
        );
        assert!(!state.is_asking_gh(), "and the spinner has stopped");
    }

    #[test]
    fn a_pull_request_gh_found_widens_the_sweep_through_the_loop() {
        let port = std::sync::Arc::new(Answering {
            answer: Some(Ok(crate::port::SettledPullRequests::All(vec![
                crate::port::SettledPullRequest {
                    number: 9,
                    head_ref: "feat/login".to_string(),
                    from_a_fork: false,
                    outcome: crate::port::PullRequestOutcome::Merged,
                },
            ]))),
            ..Answering::default()
        });
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut pending = Pending {
            dirty: Dirty::new(port.clone()),
            settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
        };
        pending.dirty.ask(state.tree());

        press(&mut state, 'S');
        settle(&mut state, &mut pending);

        assert_eq!(
            state.chosen(),
            vec![(
                RepoKey::of(&state.tree().repos[0]),
                CheckoutPath::for_test("/wt/feat-login")
            )],
            "git had nothing to say about it; gh may only widen, and this is widening"
        );
    }

    #[test]
    fn a_reload_asks_gh_again_because_a_pull_request_can_land_while_the_picker_is_up() {
        let port = std::sync::Arc::new(Answering::default());
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut pending = Pending {
            dirty: Dirty::new(port.clone()),
            settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
        };
        pending.dirty.ask(state.tree());
        press(&mut state, 'S');
        settle(&mut state, &mut pending);
        assert_eq!(port.asked().len(), 1);

        // What the `r` arm does to the sweep's half. `r` itself is `Ignored` during a sweep,
        // so this is the state the arm leaves behind rather than the key.
        pending.settled.forget();
        settle(&mut state, &mut pending);
        assert_eq!(
            port.asked().len(),
            2,
            "the reload asks again rather than showing what it had"
        );
    }

    #[test]
    fn entering_a_sweep_again_asks_gh_again_where_it_refused() {
        // A `gh` that could not answer when `Shift-S` was first pressed is not one that can
        // never answer. The only other way to ask again is `r`, which a sweep does not take
        // and the footer does not send the user out to press — so a network that was out
        // for one keypress was out for the life of the picker.
        let port = std::sync::Arc::new(Recovering::default());
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut pending = Pending {
            dirty: Dirty::new(port.clone()),
            settled: crate::app::settled::Settled::new(port.clone(), port.clone()),
        };
        pending.dirty.ask(state.tree());

        press(&mut state, 'S');
        settle(&mut state, &mut pending);
        assert_eq!(
            state.sweep_trouble(),
            Some("me/app: gh refused the question this asked: could not connect")
        );

        press(&mut state, 'S');
        press(&mut state, 'S');
        settle(&mut state, &mut pending);
        assert_eq!(
            state.sweep_trouble(),
            None,
            "asked again on the way back in, and this time it answered"
        );
        assert_eq!(port.asked(), 2, "and not on any frame between");
    }

    #[test]
    fn the_loop_keeps_its_clock_while_the_sweep_is_waiting_on_gh() {
        // `show_answers`' answer is what the loop turns the spinner on, and what makes it
        // poll instead of blocking in `event::read()`. With `gh` left out of it, `asking gh…`
        // draws frame zero for ever and the answer reaches the rows only when the user
        // happens to press a key.
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut pending = pending_on_gh(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));

        // The walk has nothing outstanding; the sweep does.
        state.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('S'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(state.is_sweeping());
        pending.settled.ask(state.tree());

        assert!(
            show_answers(&mut state, &mut pending),
            "gh is still out, so the loop still has something to wake up for"
        );
        assert!(state.is_asking_gh(), "and the prompt line says which");
    }

    #[test]
    fn a_yes_starts_the_removal_and_says_on_the_row_that_it_is_going() {
        // The arm behind `y`. Deleted, the confirmation box closes and nothing happens —
        // no error, no message, the row unchanged — and the whole gate stays green.
        let recorder = Recorder::default();
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        let removal = only_removal(&state);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &recorder,
            &Answers(Some(false)),
            &removal,
        );

        assert_eq!(recorder.did(), ["start /wt/feat-login after 0"]);
        assert_eq!(removals.paths(), [CheckoutPath::for_test("/wt/feat-login")]);
        assert!(
            state.rows().iter().any(|row| row.is_removing),
            "and the row it is happening to says so"
        );
        assert_eq!(state.message(), None, "the row is the report");
    }

    #[test]
    fn a_removal_that_will_not_start_puts_the_reason_where_the_user_is_looking() {
        // Ignoring the error is the narrower version of deleting the arm, and worse than it
        // looks: the panes are already closed by the time this can fail.
        let recorder = Recorder::default();
        let mut removals = Removals::new(&Refuses);
        let mut state = PanesState::new(no_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        let removal = only_removal(&state);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &recorder,
            &Answers(Some(false)),
            &removal,
        );

        assert_eq!(
            state.message(),
            Some(
                "could not start removing feat/login: could not spawn: no such file or \
                 directory"
            )
        );
        assert!(!state.rows().iter().any(|row| row.is_removing));
    }

    #[test]
    fn panes_that_have_certainly_closed_are_not_left_on_screen_without_a_word() {
        // The branch the other two never enter: both use a checkout with no panes, so the
        // guard's direction is pinned and its body is not. Here the panes did close, so the
        // list on screen is known wrong rather than merely stale — and when it cannot be
        // read again, saying nothing leaves rows for panes that have stopped, with the
        // cursor able to jump to them.
        let recorder = Recorder::default();
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        let removal = only_removal(&state);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &recorder,
            &Answers(Some(false)),
            &removal,
        );

        assert_eq!(
            state.message(),
            None,
            "the removal started; it is the list that could not be caught up"
        );
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
        assert!(
            state.rows().iter().any(|row| row.is_removing),
            "and the removal is still shown as going, because it is"
        );
    }

    #[test]
    fn a_close_that_stopped_partway_takes_the_panes_it_closed_off_the_list() {
        // The `Err` arm's half of the certainty the `Ok` arm already acts on: one pane is
        // gone and the other is not, so the list is known wrong rather than merely stale.
        // Left drawn, the row for the one that is gone is one the cursor stops on, and
        // `Enter` there fails `pane_focus` and carries that out of `run` through `perform`
        // — the picker disappears, with the reason only in the log.
        let herdr = Arc::new(Closing::refusing(SECOND_PANE));
        let mut removals = Removals::new(&Refuses);
        let (_, tree) = collect::collect_tree(&*herdr, &*herdr).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut dirty = Dirty::new(herdr.clone());
        let removal = removal_of(&state, FEAT_LOGIN);
        assert_eq!(pane_rows(&state), [FIRST_PANE, SECOND_PANE]);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &*herdr,
            &*herdr,
            &removal,
        );

        assert_eq!(
            state.message(),
            Some(
                "could not close w2:p2: herdr rejected pane.close: no such pane (not_found) \
                 — 1 of its 2 panes was closed first, and the checkout was not removed"
            ),
            "the account of what happened is kept whole"
        );
        assert_eq!(
            pane_rows(&state),
            [SECOND_PANE],
            "and the pane that did close is off the list without anyone pressing `r`"
        );
    }

    #[test]
    fn a_close_that_stopped_partway_says_so_when_the_list_will_not_be_read_either() {
        // Two things went wrong and the line carries both, in that order: the panes that
        // stopped are what nothing else on screen will mention, and a list that could not be
        // caught up is what the reader is about to act on. It is the order that is pinned
        // here; writing the list over the account is what `start_removal`'s last arm refuses.
        let recorder = Recorder::refusing(SECOND_PANE);
        let git = Answers(Some(false));
        let mut removals = Removals::new(&Refuses);
        let mut state = PanesState::new(two_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        let removal = only_removal(&state);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &recorder,
            &git,
            &removal,
        );

        assert_eq!(state.message(), Some("could not close w2:p2: herdr rejected pane.close: no such pane (not_found) — 1 of its 2 panes was closed first, and the checkout was not removed"));
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
    }

    #[test]
    fn a_close_that_refused_at_the_first_pane_reads_the_list_anyway() {
        // A count of none is not the same as nothing having happened. `pane_close` answers
        // `Ok` for a pane that had already gone, so an `Err` is a call this side cannot
        // account for — the pane may be closed and the row for it wrong. `Recorder` will not
        // describe itself, so the reading asked for here arrives as a clause on the end of
        // this line; with herdr answering it would cost one round trip and say nothing.
        let recorder = Recorder::refusing(FIRST_PANE);
        let git = Answers(Some(false));
        let mut removals = Removals::new(&Refuses);
        let mut state = PanesState::new(two_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        let removal = only_removal(&state);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &recorder,
            &git,
            &removal,
        );

        assert_eq!(state.message(), Some("could not close w2:p1: herdr rejected pane.close: no such pane (not_found) — none of its 2 panes were closed, and the checkout was not removed"));
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
    }

    #[test]
    fn a_working_tree_git_would_not_read_is_handed_on_as_its_own_refusal() {
        // Not the same as dirty and not the same as clean. Folded into either, a checkout
        // git could not answer for is offered for removal on the strength of an answer
        // nobody gave.
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(None)));
        dirty.ask(state.tree());

        let mut pending = pending(dirty);
        until(
            "the walk never answered for the only checkout there is",
            || !show_answers(&mut state, &mut pending),
        );

        state.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('D'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("git would not read that working tree")
        );
    }

    #[test]
    fn the_loop_hands_on_what_the_walk_answered_or_nothing_can_be_deleted() {
        // The wiring that shipped dead once, and would ship dead again silently: with the
        // answers never handed on, every checkout with panes says "still reading that
        // working tree" for the life of the picker and the feature is unreachable.
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        dirty.ask(state.tree());

        let mut pending = pending(dirty);
        until(
            "the walk never answered for the only checkout there is",
            || !show_answers(&mut state, &mut pending),
        );

        // The cursor starts on the only pane there is.
        state.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('D'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(
            state.pending_removal().is_some(),
            "the walk answered, so the question can be asked: {:?}",
            state.message()
        );
    }

    #[test]
    fn every_refusal_to_start_is_on_the_prompt_line() {
        // Two children that never ran have no toast to speak for them, so the line has to
        // carry both — one overwriting the other would lose a checkout the box listed.
        let session = Arc::new(Session::new(false, false));
        let (mut state, _pending) = sweep_asked(&session);
        let recorder = Recorder::default();
        let mut removals = Removals::new(&Refuses);
        let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        start_sweep(&mut state, &mut removals, &recorder, &sweep);

        assert_eq!(
            state.message(),
            Some(
                "could not start removing feat/login: could not spawn: no such file or \
                 directory; could not start removing fix/crash: could not spawn: no such \
                 file or directory"
            )
        );
        assert!(removals.is_empty(), "nothing started");
        assert!(!state.is_sweeping(), "and the sweep is over either way");
    }

    #[test]
    fn a_reload_that_fails_leaves_the_list_saying_it_is_behind() {
        // `r` asks no question, so its failure has nothing to withdraw: there is no account
        // of this moment to give, only the list still being what it was. That is the same
        // condition a removal's failed reading leaves, reached the other way, and it says so
        // in the same words rather than in herdr's alone.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = enter_pressed(&session);
        re_read(
            &mut state,
            &mut pending,
            &Recorder::default(),
            &*session,
            ReRead::Key,
        );
        assert_eq!(state.message(), None);
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
    }

    #[test]
    fn two_removals_reporting_in_one_frame_share_the_line() {
        // A sweep starts them together, so they can come home together; each names its own
        // checkout and neither overwrites the other.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = sweep_asked(&session);
        let port = Reports(RemovalOutcome::BranchKept("error: not fully merged".into()));
        let mut removals = Removals::new(&port);
        let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        start_sweep(&mut state, &mut removals, &Recorder::default(), &sweep);
        // Both answers in the channel before the one drain. Draining until it empties is not
        // the same test: the two can arrive in separate frames, and the second report then
        // replaces the first on the line rather than sharing it with it.
        until("both reported", || removals.reported() == 2);
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );

        let line = state.message().expect("two kept branches are said");
        for said in [
            "removed feat/login, branch kept: error: not fully merged",
            "removed fix/crash, branch kept: error: not fully merged",
        ] {
            assert!(line.contains(said), "{said:?} missing from {line:?}");
        }
        assert_eq!(
            line.matches("; ").count(),
            1,
            "one line, two reports: {line:?}"
        );
    }

    #[test]
    fn a_question_the_re_read_took_back_is_said_last() {
        // A removal started earlier reports while the sweep's box is up; the re-read that
        // follows takes the box back, and that is the one thing on the line with no toast.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = sweep_asked(&session);
        let port = Reports(RemovalOutcome::BranchKept("error: not fully merged".into()));
        let mut removals = Removals::new(&port);
        let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
        removals.remove(&Recorder::default(), &earlier).unwrap();
        until("it reported", || {
            drain_finished(
                &mut state,
                &mut pending,
                &mut removals,
                &*session,
                &*session,
            );
            removals.is_empty()
        });

        assert!(
            state.pending_sweep().is_none(),
            "the box went with the re-read"
        );
        assert_eq!(
            state.message(),
            Some(
                "removed feat/login, branch kept: error: not fully merged; the list changed \
                 while that was up — ask again"
            )
        );
    }

    #[test]
    fn a_question_taken_back_after_two_reports_is_still_said_last() {
        // Two reports and one reading, which is the frame's: the notice belongs to the frame
        // rather than to the report that happened to come last in it.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = sweep_asked(&session);
        let port = Reports(RemovalOutcome::BranchKept("error: not fully merged".into()));
        let mut removals = Removals::new(&port);
        let earlier: Vec<Removal> = state.tree().repos[0].worktrees[1..=2]
            .iter()
            .map(|worktree| Removal::sweeping("/src/app", worktree))
            .collect();
        for removal in &earlier {
            removals.remove(&Recorder::default(), removal).unwrap();
        }
        // Both answers in the channel before the one drain: reading one to find out would
        // take it, so the count is what a test can wait on.
        until("both reported", || removals.reported() == 2);
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );

        assert!(removals.is_empty(), "both came home in the one drain");
        let line = state
            .message()
            .expect("two reports and a withdrawal are said");
        assert!(
            line.ends_with(WITHDRAWN),
            "the withdrawal comes last: {line:?}"
        );
        assert_eq!(
            line.matches("; ").count(),
            2,
            "three things on one line: {line:?}"
        );
    }

    #[test]
    fn a_removal_that_ended_without_saying_what_happened_reads_the_list_again_and_says_so() {
        // Nothing can be told from the report: `Detached::wait` cannot separate a removal
        // that worked from one that did not, and the toast that could is the child's own and
        // does not come back through here. So the list is the only answer there is — read
        // again, and said to have been. Left out, the row simply stops spinning and reads as
        // a checkout that is still standing, which is the one thing nobody knows.
        // A session whose second reading differs from its first, so that the list having
        // been read again is something this can see rather than something the line claims.
        let session = Arc::new(Session::new(true, false));
        let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut pending = pending(Dirty::new(session.clone()));
        let mut removals = Removals::new(&Lost);
        let removal = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
        removals.remove(&Recorder::default(), &removal).unwrap();
        assert_eq!(pane_rows(&state), ["w1:p1"]);

        until("it reported", || {
            drain_finished(
                &mut state,
                &mut pending,
                &mut removals,
                &*session,
                &*session,
            );
            removals.is_empty()
        });

        assert_eq!(
            pane_rows(&state),
            ["w1:p1", "w2:p1"],
            "the list really was read again, and not only said to have been"
        );
        assert_eq!(
            state.message(),
            Some(
                "the removal of feat/login ended without saying what happened; the list has \
                 been read again since"
            )
        );
    }

    #[test]
    fn an_ending_nobody_could_read_and_a_list_nobody_could_read_are_both_said() {
        // The same ending against a herdr that will not describe itself. Neither reading got
        // an answer — the child's, and this side's — so the sentence saying the list has
        // been read is not said, and what could not be read is, once, on the end of the
        // line.
        let recorder = Recorder::default();
        let git = Answers(Some(false));
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut pending = pending(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));
        let mut removals = Removals::new(&Lost);
        let removal = only_removal(&state);
        removals.remove(&recorder, &removal).unwrap();

        until("it reported", || {
            drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
            removals.is_empty()
        });

        assert_eq!(state.message(), Some("the removal of feat/login ended without saying what happened — its 1 pane was closed first"));
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
    }

    #[test]
    fn a_removal_that_worked_says_when_the_list_could_not_be_caught_up_with_it() {
        // Saying nothing is the report when it worked, because the row leaves the list. When
        // the list cannot be read again the row does not leave, and saying nothing then says
        // the opposite of what happened — with the checkout's panes drawn as live rows the
        // cursor stops on, which is where `Enter` takes the picker down.
        let recorder = Recorder::default();
        let git = Answers(Some(false));
        let port = Reports(RemovalOutcome::Removed);
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut pending = pending(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));
        let mut removals = Removals::new(&port);
        let removal = only_removal(&state);
        removals.remove(&recorder, &removal).unwrap();

        until("it reported", || {
            drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
            removals.is_empty()
        });

        assert_eq!(state.message(), None);
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
    }

    #[test]
    fn what_became_of_the_list_is_said_before_the_question_that_went_with_it() {
        // Both in the one frame: an ending nobody could read, and a sweep's box taken back by
        // the reading after it. The withdrawal is about the box rather than about any
        // removal, so it stays last; ahead of the sentence about the list it would read as a
        // remark on the list instead of on the box that has just gone.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = sweep_asked(&session);
        let mut removals = Removals::new(&Lost);
        let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
        removals.remove(&Recorder::default(), &earlier).unwrap();

        until("it reported", || {
            drain_finished(
                &mut state,
                &mut pending,
                &mut removals,
                &*session,
                &*session,
            );
            removals.is_empty()
        });

        assert!(
            state.pending_sweep().is_none(),
            "the box went with the reading"
        );
        assert_eq!(
            state.message(),
            Some(
                "the removal of feat/login ended without saying what happened; the list has \
                 been read again since; the list changed while that was up — ask again"
            )
        );
    }

    #[test]
    fn a_refusal_reads_the_list_again_too_because_nothing_else_retries_it() {
        // git declined, so the refusal changes nothing by itself. What it does not know is
        // whether `start_removal`'s own reading got through when it closed this checkout's
        // panes: if it did not, rows for panes that have stopped are on screen and the
        // failure was a sentence the next keypress wiped. This frame is the only thing that
        // reads the list again without the user asking, so it reads. `Recorder` will not
        // describe itself, so that reading arrives as a clause on the end of the line.
        let recorder = Recorder::default();
        let git = Answers(Some(false));
        let port = Reports(RemovalOutcome::Refused("fatal: refused".into()));
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut pending = pending(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));
        let mut removals = Removals::new(&port);
        let removal = only_removal(&state);
        removals.remove(&recorder, &removal).unwrap();

        until("it reported", || {
            drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
            removals.is_empty()
        });

        assert_eq!(
            state.message(),
            Some("could not remove feat/login: fatal: refused — its 1 pane was closed first")
        );
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
    }

    #[test]
    fn a_question_over_a_list_this_could_not_read_goes_back_with_it() {
        // `re_read` takes a question back when its reading fails; this is the same situation
        // reached the other way. The removal that has just reported is the change the box
        // would be answered over, and the reading that would have noticed is the one that
        // failed. Left standing, `y` acts on a pane list taken before that removal — which
        // is ADR 0010's hazard, `git worktree remove` over a pane nobody closed.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = sweep_asked(&session);
        let port = Reports(RemovalOutcome::BranchKept("error: not fully merged".into()));
        let mut removals = Removals::new(&port);
        let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
        removals.remove(&Recorder::default(), &earlier).unwrap();
        assert!(state.pending_sweep().is_some(), "the box is up");

        // A herdr that will not describe itself takes the frame's one reading.
        let recorder = Recorder::default();
        let git = Answers(Some(false));
        until("it reported", || {
            drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
            removals.is_empty()
        });

        assert!(
            state.pending_sweep().is_none(),
            "the box went with the reading it was waiting on"
        );
        assert_eq!(
            state.message(),
            Some(
                "removed feat/login, branch kept: error: not fully merged; the question went back"
            ),
            "and not `WITHDRAWN`, which would say the list changed when nothing replaced it"
        );
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
    }

    #[test]
    fn two_endings_nobody_could_read_are_settled_by_the_one_list_once() {
        // The sentence is about the list, and there is one list however many came home in
        // the frame. Said per report it would say the same thing twice about one reading.
        let session = Arc::new(Session::new(false, false));
        let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut pending = pending(Dirty::new(session.clone()));
        let mut removals = Removals::new(&Lost);
        for worktree in &state.tree().repos[0].worktrees[1..=2] {
            let removal = Removal::sweeping("/src/app", worktree);
            removals.remove(&Recorder::default(), &removal).unwrap();
        }
        until("both reported", || removals.reported() == 2);
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );

        let line = state
            .message()
            .expect("two endings nobody could read are said");
        assert_eq!(
            line.matches(READ_AGAIN).count(),
            1,
            "one list, one sentence: {line:?}"
        );
        assert!(line.ends_with(READ_AGAIN), "and it comes last: {line:?}");
        assert_eq!(
            line.matches("; ").count(),
            2,
            "two reports and the one sentence: {line:?}"
        );
    }

    #[test]
    fn two_reports_over_one_reading_that_failed_say_what_failed_once() {
        // The same rule for the other sentence. Both reports are here; only one reading was
        // taken, so only one thing can be said about it.
        let session = Arc::new(Session::new(false, false));
        let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut pending = pending(Dirty::new(session.clone()));
        let mut removals = Removals::new(&Lost);
        for worktree in &state.tree().repos[0].worktrees[1..=2] {
            let removal = Removal::sweeping("/src/app", worktree);
            removals.remove(&Recorder::default(), &removal).unwrap();
        }
        until("both reported", || removals.reported() == 2);
        let recorder = Recorder::default();
        let git = Answers(Some(false));
        drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);

        let line = state
            .message()
            .expect("two endings nobody could read are said");
        assert!(
            !line.contains(STALE),
            "what became of the list is not one of the reports: {line:?}"
        );
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering"),
            "one list, one sentence — and being a condition is what makes it one, however \
             many removals reported over the reading that failed"
        );
        assert!(
            !line.contains(READ_AGAIN),
            "and not the one that says it was read: {line:?}"
        );
        assert_eq!(
            line.matches("; ").count(),
            1,
            "the two reports, and nothing else on the line: {line:?}"
        );
    }

    #[test]
    fn a_reading_that_worked_takes_back_the_sentence_saying_one_could_not() {
        // Issue #64's sequence, and the reason the list being behind is a condition rather
        // than a push. Two removals from one sweep report in different frames with no key
        // pressed between them, which is the ordinary case — ADR 0014 has the user carrying
        // on while they run. The first frame's reading fails. The second frame's works, and
        // `removal::message` has nothing to say about a removal that simply worked, so there
        // is no second sentence to write over the first: what retires it has to be the
        // reading itself.
        let session = Arc::new(Session::new(false, false));
        let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut pending = pending(Dirty::new(session.clone()));
        // The session answers as both, because the second frame's reading has to work.
        let git = session.clone();

        let mut lost = Removals::new(&Lost);
        let first = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
        lost.remove(&Recorder::default(), &first).unwrap();
        until("the first reported", || lost.reported() == 1);
        // A herdr that will not describe itself takes this frame's one reading.
        drain_finished(
            &mut state,
            &mut pending,
            &mut lost,
            &Recorder::default(),
            &*git,
        );
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering"),
            "the rows are behind, and the line says so"
        );

        let mut removed = Removals::new(&Reports(RemovalOutcome::Removed));
        let second = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[2]);
        removed.remove(&Recorder::default(), &second).unwrap();
        until("the second reported", || removed.reported() == 1);
        drain_finished(&mut state, &mut pending, &mut removed, &*session, &*git);

        assert_eq!(
            state.trouble(),
            None,
            "and the reading that put the list right is what takes the sentence back"
        );
        assert!(
            !state.message().unwrap_or_default().contains(STALE),
            "with nothing else on screen still saying it either: {:?}",
            state.message()
        );
    }

    #[test]
    fn a_sweeps_enter_still_waiting_for_its_box_goes_back_too() {
        // The third thing `cancel_removal` takes, and the one no caller can see from
        // outside: between `Enter` and the box, the question lives in the sweep rather than
        // in a `pending_*`, because `confirm_sweep_if_settled` will not put a box up over a
        // walk that has not answered. Missed, the user's `Enter` is eaten — no box ever
        // comes, the marks stay on screen, and the line says nothing about the sweep.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = enter_pressed(&session);
        assert!(
            state.pending_sweep().is_none(),
            "the box is waiting on the walk"
        );
        let mut removals = Removals::new(&Lost);
        let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
        removals.remove(&Recorder::default(), &earlier).unwrap();

        // A herdr that will not describe itself takes the frame's one reading, and says why
        // in two halves: this arm formats the error itself rather than going through
        // `could_not_read`, so it is a second place the whole chain has to reach the line.
        let git = Answers(Some(false));
        until("it reported", || {
            drain_finished(&mut state, &mut pending, &mut removals, &Chained, &git);
            removals.is_empty()
        });

        assert_eq!(state.message(), Some("the removal of feat/login ended without saying what happened; nothing was asked"), "the `Enter` went, and the line says which of the two it was: nothing was asked, so the marks stand and `Enter` again is the whole of the way back");
        assert_eq!(
            state.trouble().as_deref(),
            Some(
                "the list could not be read again: sending session.snapshot to herdr: broken pipe"
            )
        );
        settle(&mut state, &mut pending);
        assert!(
            state.pending_sweep().is_none(),
            "and no box comes up afterwards over the list nobody could read"
        );
    }

    #[test]
    fn a_shift_d_box_over_a_list_this_could_not_read_goes_back_too() {
        // The other of the two boxes. Nothing else reaches this arm: `pending_removal` and
        // `pending_sweep` are never up together, so the sweep's test cannot stand in for it.
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        dirty.ask(state.tree());
        let mut pending = pending(dirty);
        until("the walk answered", || {
            !show_answers(&mut state, &mut pending)
        });
        press(&mut state, 'D');
        assert!(state.pending_removal().is_some(), "the box is up");

        let recorder = Recorder::default();
        let git = Answers(Some(false));
        let mut removals = Removals::new(&Lost);
        let removal = only_removal(&state);
        removals.remove(&recorder, &removal).unwrap();
        until("it reported", || {
            drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);
            removals.is_empty()
        });

        assert!(state.pending_removal().is_none(), "and it went back");
        assert_eq!(state.message(), Some("the removal of feat/login ended without saying what happened — its 1 pane was closed first; the question went back"));
        assert_eq!(
            state.trouble().as_deref(),
            Some("the list could not be read again: herdr is not answering")
        );
    }

    #[test]
    fn a_checkout_that_left_the_list_is_forgotten_by_the_walk() {
        // `catch_up` hands the new tree to `Dirty`, which is the only thing that scopes the
        // walk to what is there now. Dropped, answers pile up for checkouts that have gone —
        // and a checkout that arrives is never walked at all, so its row answers "still
        // reading that working tree" for the life of the picker and can never be removed.
        let herdr = Arc::new(Closing::where_the_checkout_goes_too());
        let recorder = Recorder::default();
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);
        let (_, tree) = collect::collect_tree(&*herdr, &*herdr).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut dirty = Dirty::new(herdr.clone());
        dirty.ask(state.tree());
        until("both checkouts were walked", || {
            dirty.drain();
            dirty.answers().len() == 2
        });
        let removal = removal_of(&state, FEAT_LOGIN);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &*herdr,
            &*herdr,
            &removal,
        );

        assert_eq!(
            dirty.answers().keys().collect::<Vec<_>>(),
            [&CheckoutPath::for_test("/src/app")],
            "the checkout that went is not one the walk still has an answer about, and the \
             one that stayed is"
        );
    }

    #[test]
    fn what_herdr_said_reaches_the_line_with_its_cause_under_it() {
        // The socket adapter wraps: `sending session.snapshot to herdr` over the io error
        // that actually happened. Printed without the alternate form, the line carries the
        // wrapper and drops the only half that says what went wrong.
        let recorder = Recorder::default();
        let port = Started(&recorder);
        let mut removals = Removals::new(&port);
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut dirty = Dirty::new(std::sync::Arc::new(Answers(Some(false))));
        let removal = only_removal(&state);

        start_removal(
            &mut state,
            &mut dirty,
            &mut removals,
            &Chained,
            &Answers(Some(false)),
            &removal,
        );

        assert_eq!(state.message(), None);
        assert_eq!(
            state.trouble().as_deref(),
            Some(
                "the list could not be read again: sending session.snapshot to herdr: broken pipe"
            )
        );
    }

    #[test]
    fn a_frame_with_nothing_to_take_in_leaves_the_list_alone() {
        // The guard, read the only way it can be: a herdr that will not describe itself, so a
        // reading nobody asked for arrives as a sentence rather than as silence. Without it
        // the picker reads the whole session and every repository's refs on every tick a
        // removal is in flight for, and a box goes back on the frame after it came up.
        let recorder = Recorder::default();
        let git = Answers(Some(false));
        let mut state = PanesState::new(one_pane_tree(), None);
        let mut pending = pending(Dirty::new(std::sync::Arc::new(Answers(Some(false)))));
        let mut removals = Removals::new(&Refuses);

        drain_finished(&mut state, &mut pending, &mut removals, &recorder, &git);

        assert_eq!(
            state.message(),
            None,
            "nothing reported, so nothing was read"
        );
    }

    #[test]
    fn a_removal_that_worked_over_a_list_that_read_says_nothing_at_all() {
        // The ordinary ending, and the contract ADR 0014 and the changelog state: the row
        // leaving the list is the report. It holds only while the list keeps up, which is
        // what the other three arms of this are about — so this is the one that says what
        // "keeps up" looks like when it does.
        let herdr = Arc::new(Closing::where_the_checkout_goes_too());
        let port = Reports(RemovalOutcome::Removed);
        let mut removals = Removals::new(&port);
        let (_, tree) = collect::collect_tree(&*herdr, &*herdr).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut pending = pending(Dirty::new(herdr.clone()));
        let removal = removal_of(&state, FEAT_LOGIN);
        removals.remove(&*herdr, &removal).unwrap();

        until("it reported", || {
            drain_finished(&mut state, &mut pending, &mut removals, &*herdr, &*herdr);
            removals.is_empty()
        });

        assert_eq!(state.message(), None, "the row leaving is the whole report");
        assert_eq!(
            pane_rows(&state),
            ["w1:p9"],
            "and it left: the panes that were in it are off the list"
        );
    }

    #[test]
    fn a_sweeps_enter_still_waiting_survives_a_reading_that_worked() {
        // The positive control for the arm above. `replace_tree` leaves `confirming` alone on
        // purpose — the reading it waits on is the one that just happened — so an unrelated
        // removal reporting must not cost the user their `Enter`.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = enter_pressed(&session);
        let port = Reports(RemovalOutcome::Removed);
        let mut removals = Removals::new(&port);
        let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
        removals.remove(&Recorder::default(), &earlier).unwrap();

        until("it reported", || {
            drain_finished(
                &mut state,
                &mut pending,
                &mut removals,
                &*session,
                &*session,
            );
            removals.is_empty()
        });

        settle(&mut state, &mut pending);
        assert!(
            !box_paths(&state).is_empty(),
            "the box the `Enter` asked for still comes up: {:?}",
            state.message()
        );
    }

    #[test]
    fn a_reading_that_failed_reaches_the_line_with_its_cause_under_it_too() {
        // The arm that formats the error itself rather than going through `could_not_read`.
        // `Chained` is the only fake here whose refusal has a cause under it, and this is the
        // one place the alternate form is written out a second time.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = sweep_asked(&session);
        let mut removals = Removals::new(&Lost);
        let earlier = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
        removals.remove(&Recorder::default(), &earlier).unwrap();

        let git = Answers(Some(false));
        until("it reported", || {
            drain_finished(&mut state, &mut pending, &mut removals, &Chained, &git);
            removals.is_empty()
        });

        assert_eq!(state.message(), Some("the removal of feat/login ended without saying what happened; the question went back"));
        assert_eq!(
            state.trouble().as_deref(),
            Some(
                "the list could not be read again: sending session.snapshot to herdr: broken pipe"
            )
        );
    }

    #[test]
    fn a_removal_that_could_not_be_waited_on_reaches_the_line_whole() {
        // `Detached::wait`'s other failure, where the wait itself fails and the context names
        // the removal over what the OS said. Both halves are the report: the wrapper says
        // which removal, and the cause says why nobody can tell what became of it.
        let session = Arc::new(Session::new(false, false));
        let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut pending = pending(Dirty::new(session.clone()));
        let mut removals = Removals::new(&Unwaited);
        let removal = Removal::sweeping("/src/app", &state.tree().repos[0].worktrees[1]);
        removals.remove(&Recorder::default(), &removal).unwrap();

        until("it reported", || {
            drain_finished(
                &mut state,
                &mut pending,
                &mut removals,
                &*session,
                &*session,
            );
            removals.is_empty()
        });

        assert_eq!(
            state.message(),
            Some(
                "waiting for the removal of feat/login: No child processes (os error 10); \
                 the list has been read again since"
            )
        );
    }

    #[test]
    fn one_ending_nobody_could_read_among_reports_that_worked_is_settled_once() {
        // `unknown` is a fact about the frame, not about a report, so the sentence lands
        // after both and is said once — not attached to the report that happened to be
        // unreadable, and not repeated for the one that was not.
        let session = Arc::new(Session::new(false, false));
        let (_, tree) = collect::collect_tree(&*session, &*session).expect("the first reading");
        let mut state = PanesState::new(tree, None);
        let mut pending = pending(Dirty::new(session.clone()));
        let port = LostFirst::default();
        let mut removals = Removals::new(&port);
        for worktree in &state.tree().repos[0].worktrees[1..=2] {
            let removal = Removal::sweeping("/src/app", worktree);
            removals.remove(&Recorder::default(), &removal).unwrap();
        }
        until("both reported", || removals.reported() == 2);
        drain_finished(
            &mut state,
            &mut pending,
            &mut removals,
            &*session,
            &*session,
        );

        let line = state
            .message()
            .expect("the ending nobody could read is said");
        assert_eq!(
            line,
            "the removal of feat/login ended without saying what happened; the list has \
             been read again since",
            "the one that worked says nothing, and the list is settled once, last"
        );
    }

    #[test]
    fn a_refused_removal_stops_its_row_spinning_when_it_reports() {
        // The `deleting` note and its spinner are for a process still running; git's
        // refusal is the end of it.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = sweep_asked(&session);
        let port = Reports(RemovalOutcome::Refused("fatal: refused".into()));
        let mut removals = Removals::new(&port);
        let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        start_sweep(&mut state, &mut removals, &Recorder::default(), &sweep);
        assert!(state.rows().iter().any(|row| row.is_removing));
        until("both reported", || {
            drain_finished(
                &mut state,
                &mut pending,
                &mut removals,
                &*session,
                &*session,
            );
            removals.is_empty()
        });

        assert!(
            !state.rows().iter().any(|row| row.is_removing),
            "a refused row is an ordinary row again: {:?}",
            state.message()
        );
    }
}
