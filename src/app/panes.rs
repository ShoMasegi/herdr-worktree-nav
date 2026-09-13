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
use crate::ui::state::{Action, PanesState, WITHDRAWN};
use crate::ui::theme::Theme;

/// How long to wait for a key before turning the spinner on whatever is still coming. The
/// same tick the branches view runs on; with nothing outstanding this loop does not use one.
const TICK: std::time::Duration = std::time::Duration::from_millis(80);

/// What the picker was left wanting when it closed. The caller decides whether that means
/// switching views or exiting.
pub enum Exit {
    Closed,
    /// `None` when the cursor was not in a repository: the branches picker opens on its
    /// repository list either way.
    ShowBranches {
        repo_root: Option<String>,
    },
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
    initial_pane: Option<&str>,
    theme: &Theme,
) -> Result<Exit> {
    let (_, tree) = collect::collect_tree(herdr, git)?;
    let mut state = PanesState::new(tree, home_dir());
    // Both outlive this view: a removal started before a trip through the branches view is
    // still going, and a working tree walked once does not need walking again. Only
    // `set_removing` has to be seeded, because `show_answers` never touches it — the
    // removals are not its to know about. `set_working_trees` and `set_waiting` need no
    // seeding: `show_answers` sets both every frame, and the first frame runs before the
    // first draw.
    pending.dirty.ask(state.tree());
    state.set_removing(removals.paths());
    if let Some(pane_id) = initial_pane {
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
        terminal.draw(|frame| asked = render::draw(frame, &state, theme, Mode::Panes))?;
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

    perform(herdr, outcome)
}

/// Take in every removal that has reported back since the last frame — including from before
/// the last trip to the branches view, since the removals outlive both views and the picker
/// itself — and say on the prompt line what they said.
///
/// Out of the loop for the reason `show_answers` is: this is the arm with consequences. What
/// it says and how it says it are held by `two_removals_reporting_in_one_frame_share_the_line`
/// and `a_question_the_re_read_took_back_is_said_last`.
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
    let mut withdrawn = false;
    while let Some(finished) = removals.finished() {
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
                // Errors here are not fatal: the picker keeps showing what it had.
                if let Ok((_, tree)) = collect::collect_tree(herdr, git) {
                    withdrawn |= state.replace_tree(tree);
                    pending.dirty.ask(state.tree());
                }
            }
            // The panes are gone by now either way, so the report says so.
            Err(error) => said.push(removal::refusal(
                &format!("{error:#}"),
                finished.panes_closed,
            )),
        }
    }
    if withdrawn {
        said.push(WITHDRAWN.to_string());
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
            state.cancel_removal();
            state.set_message(match why {
                ReRead::Key => format!("{error:#}"),
                ReRead::ForSweep => {
                    format!("the list could not be read again, so nothing was asked: {error:#}")
                }
            });
        }
    }
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
/// nothing else — no error, no message, the row unchanged — which is a shape this
/// repository has shipped once already.
fn start_removal(
    state: &mut PanesState,
    dirty: &mut Dirty,
    removals: &mut Removals,
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    removal: &Removal,
) {
    match removals.remove(herdr, removal) {
        Ok(()) => {
            // The row says what is happening to it; nothing to add here.
            state.set_removing(removals.paths());
            // And only here: the panes are gone, so the list is known wrong rather than
            // merely possibly stale. An empty checkout's removal changes nothing on screen
            // yet and leaves the cursor where it was, which is where the next thing to tidy
            // up usually is.
            if !removal.panes().is_empty() {
                match collect::collect_tree(herdr, git) {
                    Ok((_, tree)) => {
                        state.replace_tree(tree);
                        dirty.ask(state.tree());
                    }
                    // Not fatal, but not silent either: rows for panes that have certainly
                    // stopped are on screen until this works.
                    Err(error) => state.set_message(format!(
                        "the panes closed, but the list could not be read again: {error:#}"
                    )),
                }
            }
        }
        // Nothing else is said or done. This is the account of what happened, and a failed
        // re-read would overwrite it with a sentence about panes that may not have closed
        // at all.
        Err(message) => state.set_message(message),
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

fn perform(herdr: &dyn HerdrPort, action: Action) -> Result<Exit> {
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
        Action::ShowBranches { repo_root } => Ok(Exit::ShowBranches { repo_root }),
        // Handled inside the loop, which is why the picker is still up after one.
        Action::Consumed
        | Action::Ignored
        | Action::Reload
        | Action::SweepReload
        | Action::RemoveWorktree { .. }
        | Action::RemoveWorktrees { .. } => Ok(Exit::Closed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fakes::{until, Recorder, Refuses, RefusesFirst, Reports, Started};
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
        assert_eq!(
            state.message(),
            Some("the list could not be read again, so nothing was asked: herdr is not answering")
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
            Some("the panes closed, but the list could not be read again: herdr is not answering"),
            "the removal started; it is the list that could not be caught up"
        );
        assert!(
            state.rows().iter().any(|row| row.is_removing),
            "and the removal is still shown as going, because it is"
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
    fn a_reload_that_fails_says_only_what_failed() {
        // `r` asks no question, so its failure has nothing to withdraw and says herdr's
        // words alone.
        let session = Arc::new(Session::new(false, false));
        let (mut state, mut pending) = enter_pressed(&session);
        re_read(
            &mut state,
            &mut pending,
            &Recorder::default(),
            &*session,
            ReRead::Key,
        );
        assert_eq!(state.message(), Some("herdr is not answering"));
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
}
