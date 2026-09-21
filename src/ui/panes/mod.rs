//! The panes picker: the list of what herdr has open, and what the keyboard does to it.
//!
//! [`PanesState`] is here with the list it draws and the cursor that walks it. The two
//! things that are modes of their own are a module each: [`sweep`] is the sweep, which
//! judges every checkout and holds what the user marked, and [`keys`] is the keymap, which
//! is pure — it maps a key and the current state to an [`Action`] — so the whole of it is
//! covered by ordinary tests.
use std::collections::{BTreeMap, BTreeSet};

use crate::domain::model::{CheckoutPath, RepoKey, Tree, WorkingTree};
use crate::domain::notice::{self, Condition};
use crate::domain::removal::{Removal, SweepRemoval};
use crate::domain::rows::{self, DisplayLine, Row, RowRef, StateFilter, ViewOptions};
use crate::domain::sweep::{Changes, RepoRoot};
use crate::port::SettledPullRequests;
use crate::ui::words;

/// What the event loop should do about a key. Anything that touches herdr is returned
/// rather than performed, so the terminal can be restored first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The key was consumed and only the display changed.
    Consumed,
    /// The key meant nothing here.
    Ignored,
    Quit,
    /// Focus this pane.
    Jump(String),
    /// Open a checkout that no workspace currently has open.
    OpenWorktree {
        repo_root: String,
        checkout_path: String,
    },
    /// Add a pane to a checkout that is already open.
    NewPane {
        checkout_path: String,
        beside_pane_id: String,
    },
    /// Show the branches picker. `None` when the cursor is not in a repository — the
    /// picker opens on its list of them either way.
    ShowBranches {
        repo_root: Option<String>,
    },
    /// Delete a checkout. The only thing this plugin does that cannot be undone by doing it
    /// again the other way.
    ///
    /// The panes it names are closed first, in the order given. A checkout with panes in it
    /// is the ordinary end state of a finished worktree, not an unusual one — see
    /// `docs/adr/0010-closing-the-panes-first.md`.
    RemoveWorktree(Removal),
    Reload,
    /// Read the tree and the working trees again, then ask the sweep's question over what
    /// is still marked. `gh` is not asked again: it only widens the sweep —
    /// `docs/adr/0011-what-may-be-swept.md`.
    SweepReload,
    /// Delete every checkout a sweep's box listed, and the branch of each that has one.
    RemoveWorktrees(SweepRemoval),
}

/// What the prompt line says when the tree changed under a question.
pub const WITHDRAWN: &str = "the list changed while that was up — ask again";

/// What [`PanesState::cancel_removal`] took back.
///
/// The difference is what the reader does next: a box is asked again by choosing the row and
/// pressing the key again, while a sweep's `Enter` leaves every mark where it was, so
/// pressing `Enter` once more is the whole of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancelled {
    /// There was nothing up to take.
    Nothing,
    /// A box the user was looking at: `Shift-D`'s, or the sweep's.
    Question,
    /// A sweep's `Enter`, taken before it could become a box.
    Enter,
}

pub struct PanesState {
    tree: Tree,
    options: ViewOptions,
    rows: Vec<Row>,
    /// Rows interleaved with the blank lines that separate groups. The cursor indexes this,
    /// not `rows`, so scrolling and drawing agree on what line 5 is.
    lines: Vec<DisplayLine>,
    cursor: usize,
    /// `/` filter mode: typing edits the query instead of running commands.
    filtering: bool,
    /// A removal waiting on a yes. Nothing on disk has been touched yet.
    pending_removal: Option<Removal>,
    /// The sweep's question waiting on a yes. Nothing on disk has been touched yet.
    pending_sweep: Option<SweepRemoval>,
    message: Option<String>,
    /// What went wrong the last time the list was read, while it is still true.
    ///
    /// A condition rather than a message: the rows on screen are behind until a reading
    /// works, and the reading that puts it right is the one that clears this —
    /// [`replace_tree`](Self::replace_tree), which is the only place a read lands. Pushed as
    /// a message it had
    /// nothing to retire it, so a frame whose reading worked sat under a sentence saying
    /// the list could not be read. Issue #64.
    stale: Option<String>,
    /// Whether the panel holding every condition in full is open.
    ///
    /// The prompt line has one line and gives its words up for a count as it narrows, so
    /// there has to be somewhere the whole sentence can still be read without leaving the
    /// picker. Not a condition itself: it is a thing the user opened, and it closes when
    /// they say so.
    showing_conditions: bool,
    /// Frame of the spinner on the rows being removed. Advanced by the loop that owns the
    /// clock, the same way the branches view does it — `domain` is not allowed to read one.
    tick: usize,
    /// Whether an answer is still on its way, which the prompt line says with a spinner.
    waiting: bool,
    /// The sweep, or `None` in the ordinary mode. Holding the changes here is what makes
    /// leaving the sweep forget them.
    sweep: Option<Sweeping>,
}

/// What the cursor is on, in terms that survive the row list being rebuilt.
enum Anchor {
    Checkout(String),
    Pane(String),
}

/// A sweep in progress: what `gh` has said, and what the user has said back. The candidates
/// themselves are not kept; `judge` works them out on every rebuild.
#[derive(Default)]
struct Sweeping {
    changes: Changes,
    settled: BTreeMap<RepoRoot, Option<SettledPullRequests>>,
    /// What went wrong asking `gh`, for the prompt line. Names the repository and says why;
    /// the rows say which checkouts it cost.
    trouble: Option<String>,
    /// Whether `gh` is still being waited on. Its own flag rather than the picker's
    /// `waiting`: the two are waited for at different times and say different sentences.
    waiting: bool,
    /// Whether the loop has been told this sweep was entered. One shot, read through
    /// [`PanesState::sweep_entered`].
    announced: bool,
    /// What was marked when `Enter` was pressed, held while the loop reads the tree and
    /// the working trees again. What is marked afterwards is the question; a row that lost
    /// its mark is named on the prompt line.
    confirming: Option<BTreeSet<(RepoKey, CheckoutPath)>>,
}

/// The answers a row puts a marker on. Clean and not-yet-answered are both absent, and that
/// is the point: they render identically, so a list rebuilt on the difference between them
/// would draw the same thing. The rows do differ, which is why nothing but
/// [`domain::rows::marks`](crate::domain::rows::marks) may read [`Row::working_tree`].
fn marked(answers: &BTreeMap<CheckoutPath, WorkingTree>) -> BTreeMap<&CheckoutPath, WorkingTree> {
    answers
        .iter()
        .filter(|(_, answer)| answer.is_drawn())
        .map(|(path, answer)| (path, *answer))
        .collect()
}

mod keys;
mod sweep;

#[cfg(test)]
pub(crate) mod fixtures;

impl PanesState {
    /// `home` shortens checkout paths to `~/...`; pass `None` to leave them absolute.
    pub fn new(tree: Tree, home: Option<String>) -> Self {
        let mut state = Self {
            tree,
            options: ViewOptions {
                home,
                ..ViewOptions::default()
            },
            rows: Vec::new(),
            lines: Vec::new(),
            cursor: 0,
            filtering: false,
            pending_removal: None,
            pending_sweep: None,
            message: None,
            stale: None,
            showing_conditions: false,
            tick: 0,
            waiting: false,
            sweep: None,
        };
        state.rebuild(None);
        state
    }

    /// Replace the tree after a reload, keeping the cursor on the row it was on when that
    /// row is still there. Answers whether a question on screen was taken back.
    pub fn replace_tree(&mut self, tree: Tree) -> bool {
        // A question on screen is about the panes the tree had when it was asked, and a `y`
        // against a list that has moved on would close panes nobody was shown. The sweep's
        // `confirming` is not taken: the re-read it waits on is what replaces the tree.
        let removal = self.pending_removal.take().is_some();
        let sweep = self.pending_sweep.take().is_some();
        let withdrawn = removal || sweep;
        if withdrawn {
            self.message = Some(WITHDRAWN.into());
        }
        // The reading that produced this tree is the one that puts the list right, so
        // whatever the last failed reading left behind stops being true here.
        self.stale = None;
        // The row, whichever kind it is. Anchored to a pane alone, a cursor on a checkout
        // was put back by line index against a list that had just got shorter — onto the
        // next checkout down, with the user's `Space` about to land on it.
        let anchor = self.anchor();
        let at = self.cursor;
        self.tree = tree;
        self.relist();
        self.restore_cursor(anchor, at);
        withdrawn
    }

    /// Say what git has said about each working tree so far. Arrives after the first frame,
    /// one answer at a time, so nothing may move under the reader: the cursor stays where it
    /// is, the row count cannot change, and the meta column is measured with room for these
    /// already kept ([`domain::rows::marks_reserve`](crate::domain::rows::marks_reserve)).
    ///
    /// One map rather than a list of the dirty ones and a list of who has answered, because
    /// the difference between "clean" and "not asked" is what decides whether somebody's
    /// panes may be closed — and two lists let a caller consult one and forget the other.
    pub fn set_working_trees(&mut self, answers: BTreeMap<CheckoutPath, WorkingTree>) {
        if self.options.working_trees == answers {
            return;
        }
        // Outside a sweep only the answers a row would draw are worth rebuilding for: a
        // checkout turning out to be clean draws the same before and after, and there are as
        // many of those as there are checkouts. A sweep is the exception, and `Clean` is why:
        // it is the answer no row draws and the answer the sweep turns into a mark.
        let redraws =
            self.sweep.is_some() || marked(&self.options.working_trees) != marked(&answers);
        self.options.working_trees = answers;
        if redraws {
            // This feeds nothing but `Row::working_tree`, so the list comes back with the
            // same length and the same order it went in with.
            self.relist();
        }
    }

    /// Whether something is still being waited for, which the prompt line turns a spinner
    /// for. Set by the loop; the state cannot see a thread any more than it can see a clock.
    pub fn set_waiting(&mut self, waiting: bool) {
        self.waiting = waiting;
    }

    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    /// Say which checkouts are being removed, so their rows can say so and stop being
    /// selectable. The removals themselves are running in other processes entirely.
    ///
    /// The cursor holds its place rather than being sent back to the top: tidying up comes
    /// in batches, and the next thing to delete is usually the next row down. It only moves
    /// when the row it is on has just become one of these.
    pub fn set_removing(&mut self, paths: Vec<CheckoutPath>) {
        if self.options.removing == paths {
            return;
        }
        // The anchor changes nothing here — the list keeps its shape — but it is passed all
        // the same, so there is one way of putting the cursor back rather than two.
        let anchor = self.anchor();
        let at = self.cursor;
        self.options.removing = paths;
        self.relist();
        self.restore_cursor(anchor, at);
    }

    /// Advance the spinner one frame. Called by the loop that owns the clock, and only
    /// while something is actually being removed.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    /// Which frame of the spinner the rows being removed should show.
    pub fn frame(&self) -> usize {
        self.tick
    }

    /// Put the cursor on a specific pane, used to open the picker on the pane you came from.
    pub fn focus_pane(&mut self, pane_id: &str) {
        if let Some(index) = self.line_of_pane(pane_id) {
            self.cursor = index;
        }
    }

    /// The line a checkout's own row is drawn on.
    fn line_of_checkout(&self, checkout_path: &str) -> Option<usize> {
        self.lines.iter().position(|line| match line {
            DisplayLine::Spacer => false,
            DisplayLine::Row(index) => match self.rows[*index].reference {
                RowRef::Worktree(r, w) => {
                    self.tree.repos[r].worktrees[w].checkout_path == checkout_path
                }
                _ => false,
            },
        })
    }

    fn line_of_pane(&self, pane_id: &str) -> Option<usize> {
        self.lines.iter().position(|line| match line {
            DisplayLine::Spacer => false,
            DisplayLine::Row(index) => match self.rows[*index].reference {
                RowRef::Pane(r, w, p) => {
                    self.tree.repos[r].worktrees[w].panes[p].pane_id == pane_id
                }
                RowRef::Ungrouped(p) => self.tree.ungrouped[p].pane_id == pane_id,
                _ => false,
            },
        })
    }

    pub fn lines(&self) -> &[DisplayLine] {
        &self.lines
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Index into [`lines`](Self::lines), not into `rows`.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn query(&self) -> &str {
        &self.options.query
    }

    pub fn state_filter(&self) -> Option<StateFilter> {
        self.options.state_filter
    }

    /// Whether worktrees that contain no pane are visible.
    pub fn shows_worktrees_without_panes(&self) -> bool {
        // ViewOptions stores the hide rule, but this API describes visible rows.
        !self.options.hide_worktrees_without_panes
    }

    /// Set visibility from the plugin configuration or the runtime `p` toggle.
    pub fn set_show_worktrees_without_panes(&mut self, show: bool) {
        if self.shows_worktrees_without_panes() == show {
            return;
        }
        let anchor = self.anchor();
        let at = self.cursor;
        self.options.hide_worktrees_without_panes = !show;
        self.relist();
        self.restore_cursor(anchor, at);
    }

    pub fn is_filtering(&self) -> bool {
        self.filtering
    }

    /// Shortens checkout paths to `~`, for anything drawn outside the rows.
    pub fn home(&self) -> Option<&str> {
        self.options.home.as_deref()
    }

    /// Take back a question that could not be asked, and say what was taken. The picker calls
    /// this when the pane is too small to draw the box — leaving `y` armed over a question
    /// nobody saw would be asking it without asking it — and when a reading a question waits
    /// on fails, since asked anyway it would be asked over the facts that reading was for
    /// replacing.
    ///
    /// Three things go, and the answer covers all three. A caller cannot see the third: a
    /// sweep's `Enter` has no accessor, and until the walk answers it is neither of the two
    /// that do.
    pub fn cancel_removal(&mut self) -> Cancelled {
        // Every take runs: `a.take().is_some() || b.take().is_some()` would skip the second.
        let removal = self.pending_removal.take().is_some();
        let sweep = self.pending_sweep.take().is_some();
        let confirming = match self.sweep.as_mut() {
            Some(sweeping) => sweeping.confirming.take().is_some(),
            None => false,
        };
        // A box and an `Enter` waiting to become one cannot both be up, so the order here
        // decides nothing; a box is named first because it is the one that was on screen.
        match (removal || sweep, confirming) {
            (true, _) => Cancelled::Question,
            (false, true) => Cancelled::Enter,
            (false, false) => Cancelled::Nothing,
        }
    }

    /// The removal being asked about, which the picker turns into a dialog.
    pub fn pending_removal(&self) -> Option<&Removal> {
        self.pending_removal.as_ref()
    }

    /// The sweep's removals being asked about, which the picker turns into a dialog.
    pub fn pending_sweep(&self) -> Option<&SweepRemoval> {
        self.pending_sweep.as_ref()
    }

    /// Say something in the search line until the next key. git says its piece over several
    /// lines and this is one, so the whitespace is collapsed on the way in.
    pub fn set_message(&mut self, message: String) {
        self.message = Some(message.split_whitespace().collect::<Vec<_>>().join(" "));
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Say that the list on screen is behind, and why, until a reading puts it right.
    ///
    /// Unlike [`set_message`](Self::set_message) this is not taken back by a keypress: the
    /// rows go on being wrong whatever the user presses, and the only thing that makes it
    /// untrue is a reading that works.
    pub fn set_stale(&mut self, words: String) {
        self.stale = Some(words.split_whitespace().collect::<Vec<_>>().join(" "));
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Every pane in the session, for the count beside the search box.
    pub fn pane_count(&self) -> usize {
        let grouped: usize = self
            .tree
            .repos
            .iter()
            .flat_map(|repo| repo.worktrees.iter())
            .map(|worktree| worktree.panes.len())
            .sum();
        grouped + self.tree.ungrouped.len()
    }

    /// The breadcrumb for the row under the cursor.
    pub fn detail(&self) -> String {
        match self.selected() {
            Some(row) => rows::detail(&self.tree, row.reference),
            None => String::new(),
        }
    }

    /// Judge, list, and go to the top — or to a pane, when the caller names one. What a query
    /// or a state filter wants: the list it returns has nothing to do with the one it
    /// replaced, so the first match is the place to be, unless the filter kept the pane the
    /// cursor was on. A list that is the same one with a change in it wants
    /// [`restore_cursor`](Self::restore_cursor) instead.
    fn rebuild(&mut self, anchor: Option<&str>) {
        self.relist();
        self.cursor = rows::next_row(&self.rows, &self.lines, 0).unwrap_or(0);
        if let Some(pane_id) = anchor {
            self.focus_pane(pane_id);
        }
    }

    /// Judge every checkout and lay the rows out again. Every change to the tree, the
    /// options or the sweep comes through here; where the cursor goes afterwards is the
    /// caller's to say.
    fn relist(&mut self) {
        self.options.sweep = self.judge();
        self.rows = rows::flatten(&self.tree, &self.options);
        self.lines = rows::display_lines(&self.rows);
    }

    /// Put the cursor back after [`relist`](Self::relist) has changed the list under it.
    ///
    /// On the row it was on, when that row is still listed and the cursor may still stop
    /// there. The row rather than the line, because the line index moves: a filter cleared
    /// makes the list longer and a checkout removed makes it shorter, and the same index then
    /// names something else. Not the row when the cursor may no longer stop on it: leaving a
    /// sweep makes a checkout with panes in it unselectable again, and a cursor left there is
    /// a highlight the arrow keys can never put back.
    ///
    /// Otherwise the nearest line at or after where it was, wrapping to the top only when
    /// nothing at or after it may be stopped on: tidying up comes in batches, and the next
    /// thing to tidy is near the last one.
    fn restore_cursor(&mut self, anchor: Option<Anchor>, at: usize) {
        self.cursor = anchor
            .and_then(|anchor| self.line_of_anchor(&anchor))
            .filter(|&line| rows::selectable(&self.rows, &self.lines, line))
            .or_else(|| {
                let from = at.min(self.lines.len().saturating_sub(1));
                rows::next_row(&self.rows, &self.lines, from)
            })
            .unwrap_or(0);
    }

    /// What the prompt line says went wrong, or nothing.
    ///
    /// git not reading a repository's refs comes first, and `gh` only after it: the first is
    /// about the track markers on every row of the repository and is true sweep or no sweep,
    /// while `gh` is asked only during a sweep and about the half git could not decide. One
    /// sentence at a time, so the `gh` one waits behind the git one until that is fixed.
    pub fn trouble(&self) -> Option<String> {
        words::conditions_line(&self.conditions())
    }

    /// Whether the panel holding every condition in full is open.
    pub fn is_showing_conditions(&self) -> bool {
        self.showing_conditions
    }

    /// Everything that is wrong right now, in full and in order.
    ///
    /// The prompt line can hold one of these; this is what the count on the end of it is
    /// standing in for, and what a reader who wants the rest is shown.
    pub fn conditions(&self) -> Vec<Condition> {
        notice::conditions(&self.tree, self.stale.as_deref(), self.sweep_trouble())
    }

    fn selected(&self) -> Option<&Row> {
        match self.lines.get(self.cursor)? {
            DisplayLine::Spacer => None,
            DisplayLine::Row(index) => self.rows.get(*index),
        }
    }

    /// What the cursor is on, said in a way that survives the list being rebuilt: a checkout
    /// by its path, a pane by its id.
    ///
    /// A line index does not survive, and clearing a filter is exactly the case where it
    /// looks as though it might: the index came from a list of four rows and is used against
    /// a list of fifteen, so the cursor lands on whatever happens to be fourth. What the user
    /// searched for is the one row they were not looking at any more.
    fn anchor(&self) -> Option<Anchor> {
        let row = self.selected()?;
        match row.reference {
            RowRef::Worktree(r, w) => Some(Anchor::Checkout(
                self.tree.repos[r].worktrees[w].checkout_path.clone(),
            )),
            _ => self
                .selected_pane_id()
                .map(|id| Anchor::Pane(id.to_string())),
        }
    }

    fn line_of_anchor(&self, anchor: &Anchor) -> Option<usize> {
        match anchor {
            Anchor::Checkout(path) => self.line_of_checkout(path),
            Anchor::Pane(pane_id) => self.line_of_pane(pane_id),
        }
    }

    fn selected_pane_id(&self) -> Option<&str> {
        match self.selected()?.reference {
            RowRef::Pane(r, w, p) => {
                Some(self.tree.repos[r].worktrees[w].panes[p].pane_id.as_str())
            }
            RowRef::Ungrouped(p) => Some(self.tree.ungrouped[p].pane_id.as_str()),
            _ => None,
        }
    }

    /// The repository the cursor is inside, whatever kind of row it is on.
    fn selected_repo_index(&self) -> Option<usize> {
        match self.selected()?.reference {
            RowRef::Repo(r) | RowRef::Worktree(r, _) | RowRef::Pane(r, _, _) => Some(r),
            _ => None,
        }
    }

    /// The worktree the cursor is inside, whether it is on the worktree row or on one of
    /// its panes.
    fn selected_worktree(&self) -> Option<(usize, usize)> {
        match self.selected()?.reference {
            RowRef::Worktree(r, w) | RowRef::Pane(r, w, _) => Some((r, w)),
            _ => None,
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.lines.is_empty() {
            return;
        }
        let len = self.lines.len();
        let start = (self.cursor as isize + delta).rem_euclid(len as isize) as usize;
        self.cursor = if delta >= 0 {
            rows::next_row(&self.rows, &self.lines, start)
        } else {
            rows::previous_row(&self.rows, &self.lines, start)
        }
        .unwrap_or(self.cursor);
    }

    /// Jump to the head of the previous or next group.
    fn move_group(&mut self, steps: isize) {
        if let Some(index) = rows::step_group(&self.rows, &self.lines, self.cursor, steps) {
            self.cursor = index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::Refs;
    use crate::port::AgentStatus;
    use crate::ui::panes::fixtures::*;
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn git_not_reading_the_refs_is_said_ahead_of_gh_and_outside_a_sweep() {
        let mut state = state();
        assert_eq!(state.trouble(), None);

        let mut tree = state.tree.clone();
        tree.repos[0].refs = Refs::Unreadable("fatal: bad ref".into());
        state.replace_tree(tree);
        assert_eq!(
            state.trouble().as_deref(),
            Some("me/app: refs unreadable: fatal: bad ref"),
            "sweep or no sweep: the track markers are missing either way"
        );

        // Both in trouble during a sweep: git's is the one about the track markers on the
        // row, so it is the one said. `gh`'s waits behind it and is counted, not dropped —
        // a line that named one of two and said nothing about the other would read as the
        // whole story, and the count is what a reader follows to the rest.
        state.handle_key(key(KeyCode::Char('S')));
        state.set_settled(
            BTreeMap::from([(RepoRoot::of(&state.tree.repos[0]), None)]),
            Some("me/app: gh could not be run".to_string()),
            false,
        );
        assert_eq!(state.sweep_trouble(), Some("me/app: gh could not be run"));
        assert_eq!(
            state.trouble().as_deref(),
            Some("me/app: refs unreadable: fatal: bad ref (+1 more)")
        );
        assert_eq!(
            state.conditions().len(),
            2,
            "and both are there in full for whoever asks for them"
        );

        let mut tree = state.tree.clone();
        tree.repos[0].refs = Refs::Read;
        state.replace_tree(tree);
        assert_eq!(
            state.trouble().as_deref(),
            Some("me/app: gh could not be run"),
            "and is what shows once git's is fixed"
        );
    }

    #[test]
    fn a_removal_starting_elsewhere_leaves_the_cursor_where_it_was() {
        // `the_cursor_steps_off_a_checkout_once_its_removal_has_started` says where the
        // cursor is not; this says where it is. The top of the picker is also "off the row",
        // so nothing there would notice.
        let mut state = state();
        select(&mut state, "fix/crash");
        state.set_removing(vec![CheckoutPath::for_test("/wt/app/feat-login")]);
        assert_eq!(cursor_label(&state), "fix/crash");
    }

    #[test]
    fn a_question_is_taken_back_when_the_list_it_was_about_moves_on() {
        // The window is ordinary: a removal reports back, the loop reads the tree again, and
        // the panes the question named are no longer what the checkout has.
        let mut state = state();
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Clean)]));
        select(&mut state, "codex");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(state.pending_removal().is_some());

        let mut grown = state.tree().clone();
        grown.repos[0].worktrees[1]
            .panes
            .push(pane("w2:p9", "zsh", AgentStatus::Unknown));
        state.replace_tree(grown);

        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("the list changed while that was up — ask again")
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Char('y'))),
            Action::Ignored,
            "and the answer to a withdrawn question is no answer at all"
        );
    }

    #[test]
    fn a_reload_that_takes_the_row_away_keeps_the_place_rather_than_the_top() {
        // `Shift-D` on a pane, `y`, and the pane the cursor was anchored to is one of the
        // ones that went. Going back to the first row costs the place on every removal, and
        // tidying up comes in batches.
        let mut state = state();
        select(&mut state, "codex");
        let before = state.cursor();

        let mut without = state.tree().clone();
        without.repos[0].worktrees[1].panes.clear();
        state.replace_tree(without);

        assert_ne!(state.cursor(), 0, "not back at the top");
        assert!(state.cursor() >= before.saturating_sub(1));
    }

    #[test]
    fn a_reload_that_takes_the_last_row_away_keeps_the_bottom_rather_than_the_top() {
        // The index the cursor had is past the end of the list that came back. Clamped to
        // the length rather than the last line, `next_row` starts at `len`, which wraps to
        // the top.
        let mut state = state();
        select(&mut state, "zsh");

        let mut without = state.tree().clone();
        without.ungrouped.clear();
        state.replace_tree(without);

        assert_eq!(
            cursor_label(&state),
            "fix/crash",
            "the new last row, not the first"
        );
    }

    #[test]
    fn a_search_still_lands_on_the_first_match_rather_than_on_an_old_index() {
        // The list a query returns has nothing to do with the one it replaced, so the same
        // numeric index is an arbitrary row rather than a place kept.
        let mut searching = state();
        select(&mut searching, "zsh");
        let at = searching.cursor();
        searching.handle_key(key(KeyCode::Char('/')));
        searching.handle_key(key(KeyCode::Char('c')));
        assert_ne!(
            searching.cursor(),
            at,
            "the filtered list is a different list"
        );
        assert_eq!(
            searching.selected().map(|row| row.label.as_str()),
            Some("claude"),
            "the first row the cursor can stop on"
        );
    }

    #[test]
    fn the_cursor_steps_off_a_checkout_once_its_removal_has_started() {
        let mut state = state();
        select(&mut state, "fix/crash");
        state.set_removing(vec![CheckoutPath::for_test("/wt/app/fix-crash")]);

        assert_ne!(
            state.selected().map(|row| row.label.as_str()),
            Some("fix/crash")
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Char('D'))),
            Action::Consumed,
            "and Shift-D cannot reach it"
        );
        assert!(state.pending_removal().is_none());
    }

    #[test]
    fn a_removal_that_finished_gives_the_row_back() {
        let mut state = state();
        state.set_removing(vec![CheckoutPath::for_test("/wt/app/fix-crash")]);
        state.set_removing(Vec::new());
        select(&mut state, "fix/crash");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(
            state.pending_removal().is_some(),
            "a refused removal leaves a checkout that can be asked about again"
        );
    }

    #[test]
    fn a_clean_answer_is_kept_without_rebuilding_the_list() {
        // Rebuilding on these would rebuild an identical list once per working tree on the
        // machine.
        let nothing = BTreeMap::new();
        let clean = answers(&[("/wt/app/feat-login", WorkingTree::Clean)]);
        assert_eq!(marked(&nothing), marked(&clean), "no marker either way");

        // And the ones that do draw are not folded in with them.
        for answer in [WorkingTree::Dirty, WorkingTree::Unreadable] {
            assert_ne!(
                marked(&nothing),
                marked(&answers(&[("/wt/app/feat-login", answer)])),
                "{answer:?} puts a marker on the row"
            );
        }

        // Nor with each other: keeping only *which* rows draw a marker would let a checkout
        // go on saying it holds uncommitted work after git has said it cannot read the
        // working tree at all.
        assert_ne!(
            marked(&answers(&[("/wt/app/feat-login", WorkingTree::Dirty)])),
            marked(&answers(&[("/wt/app/feat-login", WorkingTree::Unreadable)])),
            "one marker is not the other"
        );
    }

    #[test]
    fn panes_that_are_not_in_a_repository_are_always_listed() {
        // They are still panes. A picker that hides some of them makes you wonder which.
        assert!(row_labels(&state()).contains(&"zsh".to_string()));
    }

    #[test]
    fn reloading_keeps_the_cursor_on_the_same_pane() {
        let mut state = state();
        select(&mut state, "codex");
        let before = state.cursor();
        let tree = state.tree().clone();
        state.replace_tree(tree);
        assert_eq!(state.cursor(), before);
    }

    #[test]
    fn the_breadcrumb_follows_the_cursor() {
        let mut state = state();
        select(&mut state, "fix/crash");
        assert_eq!(state.detail(), "me/app · fix/crash · /wt/app/fix-crash");
    }

    #[test]
    fn the_count_beside_the_search_box_includes_panes_outside_a_repository() {
        assert_eq!(state().pane_count(), 3);
    }
}
