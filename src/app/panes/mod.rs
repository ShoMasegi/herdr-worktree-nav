//! The panes picker: draw, read a key, and act on what it meant.

use anyhow::Result;
use ratatui::crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::app::collect;
use crate::app::dirty::Dirty;
use crate::app::home_dir;
use crate::app::removals::Removals;
use crate::app::words;
use crate::app::Pending;
use crate::domain::removal::{Removal, SweepRemoval};
use crate::port::{GitPort, HerdrPort, PaneSplit, SplitDirection, WorktreeOpen};
use crate::ui::panes::{Action, Cancelled, PanesState, WITHDRAWN};
use crate::ui::render;
use crate::ui::theme::Theme;

/// How long to wait for a key before turning the spinner on whatever is still coming. The
/// same tick the branches view runs on; with nothing outstanding this loop does not use one.
const TICK: std::time::Duration = std::time::Duration::from_millis(80);

/// What the line says when the list could not be read again after something that changed it
/// or may have.
///
/// A condition, not a report: it is true of the rows on screen for as long as no reading
/// works, whatever the user presses in the meantime, and the reading that puts the list
/// right is what takes it back. So it is held by
/// [`PanesState::set_stale`](crate::ui::panes::PanesState::set_stale) and drawn beside
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
/// `worktree.list` refused loses its rows the same way, though the prompt line now says so.
/// Issues #56 and #33.
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
/// it back on every path out, so a failure surfaces as text rather than as a corrupted
/// screen.
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
    // Both outlive this view, so both are picked up rather than started again. Only
    // `set_removing` has to be seeded, because `show_answers` never touches it — it sets
    // the working trees and the spinner itself, every frame, before the first draw.
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
        // Everything the loop is waiting on, not only the removals: this is both what
        // advances the spinner and what makes the loop poll instead of blocking on a key.
        let waiting = !removals.is_empty() || still_coming;
        if waiting && last_tick.elapsed() >= TICK {
            state.tick();
            last_tick = std::time::Instant::now();
        }
        let mut asked = true;
        terminal.draw(|frame| asked = render::panes::draw(frame, &state, options.theme))?;
        if !asked {
            // The pane is too small to put the question in. Taking it back is the only
            // honest answer: the alternative is `y` armed over a box nobody ever saw.
            state.cancel_removal();
            state.set_message("this pane is too small to ask that safely".into());
        }

        drain_finished(&mut state, pending, removals, herdr, git);

        if waiting && !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        match state.handle_key(key) {
            Action::Consumed | Action::Ignored => {}
            Action::Reload => re_read(&mut state, pending, herdr, git, ReRead::Key),
            // A sweep's `Enter`: `show_answers` puts the question up once the walk answers.
            Action::SweepReload => re_read(&mut state, pending, herdr, git, ReRead::ForSweep),
            // Deleting is housekeeping, and housekeeping comes in batches: the picker stays
            // open on the list the deletion is changing rather than closing over it. See
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
            action => {
                if let Some(exit) = act(&mut state, pending, herdr, git, action) {
                    break exit;
                }
            }
        }
    };

    Ok(outcome)
}

/// Carry out the key that would close the picker, or say why it could not and stay up.
///
/// Every row is drawn from a tree read at some point in the past, so herdr declining a focus
/// or an open is the ordinary end of a row the list has not caught up with — a pane that has
/// closed, a checkout that has gone. That is something the user asked for and did not get,
/// so it is said where they are looking, and the picker is still there to say it on: leaving
/// is the right answer for a key that worked. Carried out of `run` by `?`, the only account
/// of it was `herdr plugin log list`, and from the user's seat the picker had vanished.
/// Issue #65.
///
/// `None` when the picker stayed up.
fn act(
    state: &mut PanesState,
    pending: &mut Pending,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    action: Action,
) -> Option<Exit> {
    let asked = asked_for(&action);
    match finish(herdr, action, state) {
        Ok(exit) => Some(exit),
        Err(error) => {
            state.set_message(format!("{asked}: {error:#}"));
            // The row it was tried on is evidence the list is behind, and this is the
            // reading that would put it right — a condition when it cannot be made, the way
            // a removal's is.
            if let Err(error) = catch_up(state, &mut pending.dirty, herdr, git) {
                state.set_stale(could_not_read(&error));
            }
            None
        }
    }
}

/// What the user asked for, in the words of the row they asked it on.
///
/// herdr's message names the call it refused, which is a sentence about the API and not
/// about the list: the half that says which row this was about has to come from here.
fn asked_for(action: &Action) -> &'static str {
    match action {
        Action::Jump(_) => "could not go to that pane",
        Action::OpenWorktree { .. } => "could not open that checkout",
        Action::NewPane { .. } => "could not add a pane there",
        // Nothing else asks herdr for anything, so nothing else can arrive here having
        // failed. A sentence is cheaper than a shape that makes that unsayable.
        _ => "that did not work",
    }
}

/// Take in every removal that has reported back since the last frame — including from before
/// the last trip to the branches view, since the removals outlive both views and the picker
/// itself — and say on the prompt line what they said.
fn drain_finished(
    state: &mut PanesState,
    pending: &mut Pending,
    removals: &mut Removals,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
) {
    // Several can report in one frame — a sweep starts them together — and each names its
    // own checkout, so they share the line rather than overwrite it. A question the re-read
    // below takes back is said last: the reports have their toasts, the withdrawal has
    // nothing else.
    let mut said: Vec<String> = Vec::new();
    let mut reported = false;
    // Whether any of them left nobody able to say what became of the checkout.
    let mut unknown = false;
    while let Some(finished) = removals.finished() {
        reported = true;
        state.set_removing(removals.paths());
        match finished.outcome {
            Ok(outcome) => {
                // Nothing to say when it worked: the row leaving the list is the report,
                // and the toast has already said it to whoever was not looking.
                if let Some(message) =
                    words::message(&finished.label, &outcome, finished.panes_closed)
                {
                    said.push(message);
                }
            }
            // Nobody knows how this one ended: `Detached::wait` is explicit that the removal
            // may well have happened, and the toast that would have said so is the child's
            // own. The panes are gone either way, which `refusal` adds.
            Err(error) => {
                unknown = true;
                said.push(words::refusal(&format!("{error:#}"), finished.panes_closed));
            }
        }
    }
    // Once, after all of them, because there is one list: read per removal, the line is
    // left to reconcile a reading that failed with a later one that worked about the same
    // list.
    //
    // Whatever they reported, a refusal included. A reading `start_removal` could not finish
    // leaves rows standing for panes that have stopped, and this is the only thing that
    // tries again on a frame nobody pressed a key for.
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
                // back: the removal that has just reported is the change the box would be
                // answered over, and the reading that would have noticed is the one that
                // failed.
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
/// are the user asking, and what they get is every working tree walked again. This is the
/// picker catching up with itself, so the working trees are only brought up to the new tree:
/// a removal is no reason to doubt an answer git has already given about anything else.
///
/// `Err` is herdr's own words and only ever herdr's: the one `?` inside `collect_tree` is the
/// session, and every git call under it keeps what it can and drops the rest. It is handed
/// back rather than put on the line, because what to say about a reading that failed depends
/// on what else the frame has to say.
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
/// behind it, since the child never ran — and none stops the rest: ADR 0011 has a refused
/// checkout reported on its own while the sweep carries on. No re-read afterwards, because a
/// swept checkout has no panes and nothing on screen is known wrong.
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
/// so nothing in it is reachable from a test, and this is the arm with consequences.
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
    // `start_sweep` does: a removal that never started pushed nothing.
    state.set_removing(removals.paths());

    // Wherever the removal named panes, and whatever became of it. A `pane_close` that
    // failed is no evidence the pane survived, so every way round a row may be standing for
    // a pane that has stopped — and `Enter` on such a row takes the whole picker down, since
    // `pane_focus` fails and the `?` in `perform` carries that out of `run`. An empty
    // checkout's removal changes nothing on screen yet, and leaves the cursor where the next
    // thing to tidy up usually is.
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
/// none, and it is exactly the answer that turns a refusal into an offer.
fn show_answers(state: &mut PanesState, pending: &mut Pending) -> bool {
    let Pending { dirty, settled } = pending;
    dirty.drain();
    state.set_working_trees(dirty.answers());
    let reading = dirty.is_waiting();
    state.set_waiting(reading);

    // Asked from here rather than from the key that enters the sweep because `ask` is the
    // same question every time and answers it once: a `Tab` away and back, or a sweep left
    // and re-entered, costs a map lookup rather than another round of `gh`.
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

    // Both, because the loop's clock has to run while either is out.
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
mod tests;
