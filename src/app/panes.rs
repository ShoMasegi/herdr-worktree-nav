//! The panes picker: draw, read a key, and act on what it meant.

use anyhow::Result;
use ratatui::crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::app::collect;
use crate::app::dirty::Dirty;
use crate::app::home_dir;
use crate::app::removals::Removals;
use crate::app::Pending;
use crate::domain::removal::{self, Removal};
use crate::port::{GitPort, HerdrPort, PaneSplit, SplitDirection, WorktreeOpen};
use crate::ui::render::{self, Mode};
use crate::ui::state::{Action, PanesState};
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

        // Whatever has reported back — including from before the last trip to the branches
        // view, since the removals outlive both views and the picker itself.
        while let Some(finished) = removals.finished() {
            state.set_removing(removals.paths());
            match finished.outcome {
                Ok(outcome) => {
                    // Nothing to say when it worked: the row leaving the list is the report,
                    // and the toast has already said it to whoever was not looking.
                    if let Some(message) =
                        removal::message(&finished.label, &outcome, finished.panes_closed)
                    {
                        state.set_message(message);
                    }
                    // Errors here are not fatal: the picker keeps showing what it had.
                    if let Ok((_, tree)) = collect::collect_tree(herdr, git) {
                        state.replace_tree(tree);
                        pending.dirty.ask(state.tree());
                    }
                }
                // The panes are gone by now either way, so the report says so.
                Err(error) => state.set_message(removal::refusal(
                    &format!("{error:#}"),
                    finished.panes_closed,
                )),
            }
        }

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
            // `r` is the only thing that asks about the working trees again, so a reload
            // that quietly does nothing is a reload the user reads as "still dirty, then".
            Action::Reload => match collect::collect_tree(herdr, git) {
                Ok((_, tree)) => {
                    state.replace_tree(tree);
                    // Reload means reload: whether a checkout is dirty is a fact about a
                    // working tree the user has been editing since it was last asked, and a
                    // pull request can land while the picker is up.
                    pending.dirty.reask(state.tree());
                    pending.settled.forget();
                    state.set_working_trees(pending.dirty.answers());
                }
                Err(error) => state.set_message(format!("{error:#}")),
            },
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
            action => break action,
        }
    };

    perform(herdr, outcome)
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

/// Tell the state what the walk and `gh` have said since the last frame, and answer whether
/// either has more to come.
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
        Action::Consumed | Action::Ignored | Action::Reload | Action::RemoveWorktree { .. } => {
            Ok(Exit::Closed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fakes::{until, Recorder, Refuses, Started};
    use crate::ui::state::PanesState;
    use anyhow::Result;

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
        fn local_refs(&self, _repo_root: &str) -> Result<Vec<crate::port::GitRef>> {
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
        fn local_refs(&self, _repo_root: &str) -> Result<Vec<crate::port::GitRef>> {
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
        fn local_refs(&self, _repo_root: &str) -> Result<Vec<crate::port::GitRef>> {
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
        fn local_refs(&self, _repo_root: &str) -> Result<Vec<crate::port::GitRef>> {
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
        use crate::domain::model::{PaneNode, RepoNode, Tree, WorktreeNode};
        Tree {
            repos: vec![RepoNode {
                repo_key: "/src/app/.git".into(),
                repo_root: "/src/app".into(),
                display_name: "me/app".into(),
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
            vec!["/wt/feat-login".to_string()],
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
        assert_eq!(removals.paths(), ["/wt/feat-login".to_string()]);
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
}
