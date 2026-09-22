//! Picker state and key handling.
//!
//! Key handling is pure — it maps a key and the current state to an [`Action`] — so the
//! whole keymap is covered by ordinary tests.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::model::{CheckoutPath, RepoKey, Tree, WorkingTree};
use crate::domain::notice::{self, Condition};
use crate::domain::removal::{Removal, SweepRemoval};
use crate::domain::rows::{self, DisplayLine, Row, RowRef, StateFilter, ViewOptions};
use crate::domain::sweep::{self, Changes, Mark, RepoRoot};
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

    /// What a sweep would do with every checkout, or `None` when no sweep is on.
    ///
    /// Run on every rebuild rather than kept, because everything it decides on moves while
    /// the sweep is on screen: a working tree answers, a removal starts, `gh` lands, `r`
    /// reads the tree again. The user's changes are what is kept; the judgement is not.
    fn judge(&self) -> Option<BTreeMap<(RepoKey, CheckoutPath), Mark>> {
        let sweeping = self.sweep.as_ref()?;
        let candidates = sweep::candidates(
            &self.tree,
            &sweep::Facts {
                working_trees: &self.options.working_trees,
                settled: &sweeping.settled,
                removing: &self.options.removing,
            },
        );
        // Filtered here rather than where the tree is set, so no call site has to remember
        // it: this is the only place a `Mark` is made, and `chosen` reads the marks.
        Some(sweep::marks(
            &candidates,
            &sweeping.changes.still_about(&self.tree),
        ))
    }

    /// Whether a sweep is on. The prompt line and the gutter both change with it.
    pub fn is_sweeping(&self) -> bool {
        self.sweep.is_some()
    }

    /// What `gh` has said, and what went wrong saying it.
    ///
    /// Ignored outside a sweep: the answers are asked for on entering one and would have
    /// nowhere to be shown otherwise. Landing them is a rebuild, since a row that could not
    /// be judged a moment ago now says which pull request decided it.
    pub fn set_settled(
        &mut self,
        settled: BTreeMap<RepoRoot, Option<SettledPullRequests>>,
        trouble: Option<String>,
        waiting: bool,
    ) {
        let Some(sweeping) = self.sweep.as_mut() else {
            return;
        };
        let unchanged = sweeping.settled == settled
            && sweeping.trouble == trouble
            && sweeping.waiting == waiting;
        if unchanged {
            return;
        }
        sweeping.settled = settled;
        sweeping.trouble = trouble;
        sweeping.waiting = waiting;
        // The cursor is not touched: this feeds nothing but `Row::sweep`, so the list that
        // comes back is the same length in the same order.
        self.relist();
    }

    /// What went wrong asking `gh`, for the prompt line.
    pub fn sweep_trouble(&self) -> Option<&str> {
        self.sweep.as_ref()?.trouble.as_deref()
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

    /// Whether a sweep has been entered since this was last asked. True once per entry.
    ///
    /// The loop reads it on the frame after `Shift-S` and asks `gh` again where it refused
    /// last time —
    /// [`Settled::forget_failures`](crate::app::settled::Settled::forget_failures).
    /// Asked here rather than answered by the key, because what has to happen is not herdr's
    /// to perform: it is a change to what the loop is waiting on, which the loop owns and this
    /// cannot see.
    pub fn sweep_entered(&mut self) -> bool {
        let Some(sweeping) = self.sweep.as_mut() else {
            return false;
        };
        !std::mem::replace(&mut sweeping.announced, true)
    }

    /// Whether `gh` is still out. The prompt line turns a spinner for it, because until it
    /// answers the rows are showing what git alone decided — which is a smaller sweep than
    /// the one the user is about to get, and it is about to change under their cursor.
    pub fn is_asking_gh(&self) -> bool {
        self.sweep.as_ref().is_some_and(|sweeping| sweeping.waiting)
    }

    /// How many checkouts carry a mark. Shown where the pane count is outside a sweep,
    /// because during one it is the number the user is deciding about.
    pub fn marked_count(&self) -> usize {
        self.options
            .sweep
            .as_ref()
            .map(|marks| marks.values().filter(|mark| mark.is_going()).count())
            .unwrap_or(0)
    }

    /// Every checkout the sweep would remove, in repository and path order.
    ///
    /// The answer `Enter` acts on, read again after the re-read it asks for. Empty outside a
    /// sweep, which is not the same as "nothing is marked" but leads to the same place:
    /// nothing is removed.
    pub fn chosen(&self) -> Vec<(RepoKey, CheckoutPath)> {
        let Some(marks) = self.options.sweep.as_ref() else {
            return Vec::new();
        };
        marks
            .iter()
            .filter(|(_, mark)| mark.is_going())
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Enter the sweep, or leave it.
    ///
    /// Leaving forgets the changes: ADR 0011's "nothing is deleted that was not on the screen
    /// with a mark against it" is about the screen the user is looking at, and marks that
    /// outlived a trip through the branches view would be marks they last saw some time ago.
    fn set_sweeping(&mut self, sweeping: bool) -> Action {
        self.sweep = sweeping.then(Sweeping::default);
        // No question outlives the mode it was asked in.
        self.pending_sweep = None;
        if sweeping {
            // Everything the sweep judges has to be on the screen it judged it on — ADR 0011
            // says so in those words. `judge` walks the tree while the rows walk the filtered
            // list, so with no query and no state filter `flatten`'s only other drop is the
            // no-pane hide, which a sweep disables. `/` and the state keys are `Ignored` while
            // a sweep is on, so the list cannot become filtered again underneath one.
            self.options.query.clear();
            self.options.state_filter = None;
        }
        let anchor = self.anchor();
        let at = self.cursor;
        self.relist();
        self.restore_cursor(anchor, at);
        Action::Consumed
    }

    /// Put the sweep's question up, once the re-read `Enter` asked for has answered.
    ///
    /// Called every frame by the loop; nothing happens until `Enter` was pressed and the
    /// walk is no longer waiting. The box lists what is marked *now*, and a row the re-read
    /// unmarked is named on the prompt line: it was the user's decision, and a fact
    /// overruled it — issue #25.
    pub fn confirm_sweep_if_settled(&mut self) {
        if self.waiting {
            return;
        }
        let Some(before) = self
            .sweep
            .as_mut()
            .and_then(|sweeping| sweeping.confirming.take())
        else {
            return;
        };
        let after = self.chosen();
        let still: BTreeSet<&(RepoKey, CheckoutPath)> = after.iter().collect();
        let dropped: Vec<String> = before
            .iter()
            .filter(|key| !still.contains(key))
            .map(|key| self.label_of(key))
            .collect();
        if !dropped.is_empty() {
            let named = format!("no longer marked: {}", dropped.join(", "));
            self.message = Some(match after.is_empty() {
                true => format!("{named} — nothing left to remove"),
                false => named,
            });
        }
        if !after.is_empty() {
            // The question is the one thing on screen that a key can answer destructively,
            // so it takes the space and the keyboard from a panel that was only being read.
            // Both are drawn over the list, and two boxes at once is a box nobody saw.
            self.showing_conditions = false;
            self.pending_sweep = Some(SweepRemoval::of(&self.tree, &after));
        }
    }

    /// What the row for this key is called, or its path when the tree no longer has it.
    fn label_of(&self, key: &(RepoKey, CheckoutPath)) -> String {
        match self.tree.find_checkout(key) {
            Some((_, worktree)) => worktree.label().to_string(),
            None => key.1.as_str().to_string(),
        }
    }

    /// Add or remove the mark on the row under the cursor.
    fn flip_mark(&mut self) -> Action {
        let Some(row) = self.selected() else {
            return Action::Consumed;
        };
        let RowRef::Worktree(repo, worktree) = row.reference else {
            // A group or a pane. Not a checkout, so there is nothing here to sweep, and
            // saying so on every stray `Space` would be noise.
            return Action::Consumed;
        };
        let checkout = &self.tree.repos[repo].worktrees[worktree];
        let key = (
            RepoKey::of(&self.tree.repos[repo]),
            CheckoutPath::of(checkout),
        );
        let Some(mark) = self
            .options
            .sweep
            .as_ref()
            .and_then(|marks| marks.get(&key))
        else {
            return Action::Consumed;
        };
        // The refusal is the whole message. Which checkout it is about is the row the cursor
        // is on, and the user is looking at it.
        if let Some(refusal) = mark.refusal() {
            self.message = Some(refusal.to_string());
            return Action::Consumed;
        }
        // The whole checkout, not its path: what the user said is about the branch the row
        // was showing, and a path is reused.
        let answer = mark.clone();
        let checkout = self.tree.repos[repo].worktrees[worktree].clone();
        if let Some(sweeping) = self.sweep.as_mut() {
            sweeping
                .changes
                .flip(&self.tree.repos[repo], &checkout, &answer);
        }
        self.relist();
        Action::Consumed
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

    /// What Enter means on the current row.
    ///
    /// Every row the cursor can reach stands for somewhere to go, so this is total in the
    /// only two ways that matter: go to a pane, or open a checkout that has none.
    fn activate(&mut self) -> Action {
        let Some(row) = self.selected() else {
            return Action::Consumed;
        };
        match row.reference {
            RowRef::Pane(r, w, p) => {
                Action::Jump(self.tree.repos[r].worktrees[w].panes[p].pane_id.clone())
            }
            RowRef::Ungrouped(p) => Action::Jump(self.tree.ungrouped[p].pane_id.clone()),
            RowRef::Worktree(r, w) => {
                let repo = &self.tree.repos[r];
                let worktree = &repo.worktrees[w];
                match worktree.panes.first() {
                    // A checkout that is already being worked in: go to the work rather
                    // than opening a second copy of it. Unreachable while the cursor stops
                    // only on idle checkouts, but the panes are right there either way.
                    Some(pane) => Action::Jump(pane.pane_id.clone()),
                    None => Action::OpenWorktree {
                        repo_root: repo.repo_root.clone(),
                        checkout_path: worktree.checkout_path.clone(),
                    },
                }
            }
            // Headings. The cursor does not stop on them.
            RowRef::Repo(_) | RowRef::UngroupedRepo => Action::Consumed,
        }
    }

    /// What `Shift-D` means: offer to delete the checkout under the cursor — or, on a pane,
    /// the checkout that pane is in, which is how a checkout with panes is reached at all,
    /// since the cursor does not stop on one.
    ///
    /// The refusals happen here rather than after the question, because asking "are you
    /// sure?" about something that cannot happen is worse than saying so — and because for
    /// a checkout with panes, "after the question" is after they have been closed.
    fn ask_to_remove(&mut self) -> Action {
        let Some((r, w)) = self.selected_worktree() else {
            self.message = Some("select a checkout, or a pane in one".into());
            return Action::Consumed;
        };
        let repo = &self.tree.repos[r];
        let worktree = &repo.worktrees[w];
        if worktree.is_primary {
            // git cannot remove the main working tree, and it is not a worktree anyway.
            self.message = Some("that is the repository itself, not a worktree".into());
            return Action::Consumed;
        }
        if self.options.removing.contains(&CheckoutPath::of(worktree)) {
            // A second one would race the first, and would close panes that the first is
            // already having removed out from under them.
            self.message = Some("that checkout is already being removed".into());
            return Action::Consumed;
        }
        // The refusals below are only reached by a checkout with panes in it, and that is
        // the whole reason they exist: for an empty one there is nothing to lose by letting
        // git answer for itself, but here the panes are gone by the time it speaks.
        if !worktree.panes.is_empty() {
            // `None` is not `Clean`: only an answer is a licence to close somebody's panes.
            // The walk lands after the first frame, so `None` is the ordinary state of the
            // checkout the picker opens on — the one the cursor is already sitting in.
            let refusal = match self.options.working_trees.get(&CheckoutPath::of(worktree)) {
                Some(WorkingTree::Clean) => None,
                Some(WorkingTree::Dirty) => {
                    Some("that checkout is holding work nobody has committed")
                }
                Some(WorkingTree::Unreadable) => Some("git would not read that working tree"),
                None => Some("still reading that working tree — try again"),
            };
            if let Some(refusal) = refusal {
                self.message = Some(refusal.into());
                return Action::Consumed;
            }
        }
        self.pending_removal = Some(Removal::of(&repo.repo_root, worktree));
        Action::Consumed
    }

    /// What `n` means on the current row: add a pane to this checkout.
    fn new_pane(&mut self) -> Action {
        let Some((r, w)) = self.selected_worktree() else {
            self.message = Some("select a worktree or a pane first".into());
            return Action::Consumed;
        };
        let worktree = &self.tree.repos[r].worktrees[w];
        match worktree.panes.first() {
            Some(pane) => Action::NewPane {
                checkout_path: worktree.checkout_path.clone(),
                beside_pane_id: pane.pane_id.clone(),
            },
            // With no pane there is nothing to split, so this is the same as opening it.
            None => Action::OpenWorktree {
                repo_root: self.tree.repos[r].repo_root.clone(),
                checkout_path: worktree.checkout_path.clone(),
            },
        }
    }

    /// Narrow to one agent state, or clear the filter. Mirrors the navigator's b/w/i/d/a.
    fn set_state_filter(&mut self, filter: Option<StateFilter>) -> Action {
        // Pressing the same key again clears, so a filter is never a one-way door.
        self.options.state_filter = if self.options.state_filter == filter {
            None
        } else {
            filter
        };
        let anchor = self.selected_pane_id().map(str::to_string);
        self.rebuild(anchor.as_deref());
        Action::Consumed
    }

    /// What `!` means: put every condition on screen in full.
    ///
    /// With nothing wrong it says so rather than opening an empty panel. The user pressed a
    /// key to ask a question, and "nothing" is an answer to it; a panel with no lines in it
    /// is not.
    fn show_conditions(&mut self) -> Action {
        if self.conditions().is_empty() {
            self.message = Some("nothing is wrong".into());
        } else {
            self.showing_conditions = true;
        }
        Action::Consumed
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Windows sends both press and release; only act on press.
        if key.kind == KeyEventKind::Release {
            return Action::Ignored;
        }
        self.message = None;

        // A question is on screen and it is the only thing the keyboard is for.
        if let Some(removal) = self.pending_removal.take() {
            return match key.code {
                KeyCode::Char('y') => Action::RemoveWorktree(removal),
                // Anything else is a no. Taking the removal above is what makes that true
                // of keys nobody thought of as well as of the ones they did.
                _ => Action::Consumed,
            };
        }

        // The sweep's question, under the same rule.
        if let Some(sweep) = self.pending_sweep.take() {
            return match key.code {
                KeyCode::Char('y') => {
                    // The sweep ends with its question: what `y` removes is what the box
                    // listed, and marks left behind would be marks the box was not about.
                    self.set_sweeping(false);
                    Action::RemoveWorktrees(sweep)
                }
                _ => Action::Consumed,
            };
        }

        // The panel is over the list and holds the whole of what the line could only
        // summarise, so while it is up it is what the keyboard is for. It closes on the key
        // that opened it, and on the keys that mean "go back" everywhere else in the picker.
        if self.showing_conditions {
            match key.code {
                // Abandoning the picker outright is not a key the panel is allowed to
                // swallow: it is how a pane is got rid of when nothing else is working.
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Action::Quit
                }
                KeyCode::Char('!') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.showing_conditions = false
                }
                // Anything else is left alone rather than acted on: a key pressed at a panel
                // was aimed at the panel, and the list under it is not what the user is
                // looking at.
                _ => {}
            }
            return Action::Consumed;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') => Action::Quit,
                KeyCode::Char('n') => {
                    self.move_cursor(1);
                    Action::Consumed
                }
                KeyCode::Char('p') => {
                    self.move_cursor(-1);
                    Action::Consumed
                }
                // The navigator clears its search with ctrl+u.
                KeyCode::Char('u') if self.filtering => {
                    self.options.query.clear();
                    self.rebuild(None);
                    Action::Consumed
                }
                _ => Action::Ignored,
            };
        }

        if self.filtering {
            return self.handle_filter_key(key);
        }

        // A sweep is a mode, and the keys that mean something in it are its own. Only the
        // ones that move the cursor are shared, because a list you cannot walk is a list you
        // cannot decide about.
        if self.is_sweeping() {
            match key.code {
                // Leaving is not quitting. `q` out of a sweep puts the picker back rather
                // than closing it, because the sweep is what the user opened last and it is
                // what they are getting out of.
                KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('S') => {
                    return self.set_sweeping(false)
                }
                KeyCode::Char(' ') => return self.flip_mark(),
                // Reading is not deciding, so this one key is shared with the ordinary
                // mode: `gh` refusing is a sweep's own condition, and a sweep is when the
                // user most needs to read it whole.
                KeyCode::Char('!') => return self.show_conditions(),
                KeyCode::Enter => {
                    let chosen = self.chosen();
                    if chosen.is_empty() {
                        self.message = Some("nothing is marked".into());
                        return Action::Consumed;
                    }
                    // Not asked yet: the marks rest on facts from when the picker opened
                    // or was last reloaded, and the loop reads them again first —
                    // `confirm_sweep_if_settled` asks once the walk has answered. Issue #25.
                    if let Some(sweeping) = self.sweep.as_mut() {
                        sweeping.confirming = Some(chosen.into_iter().collect());
                    }
                    return Action::SweepReload;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.move_cursor(1);
                    return Action::Consumed;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_cursor(-1);
                    return Action::Consumed;
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    self.move_group(1);
                    return Action::Consumed;
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    self.move_group(-1);
                    return Action::Consumed;
                }
                // Everything else, deliberately. A sweep is the one mode where a key that
                // half works is dangerous: `D` here would ask about one checkout while the
                // marks say something about twenty, and `Tab` would leave marks behind on a
                // screen the user has left.
                _ => return Action::Ignored,
            }
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                Action::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                Action::Consumed
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.move_group(1);
                Action::Consumed
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.move_group(-1);
                Action::Consumed
            }
            KeyCode::Enter => self.activate(),
            KeyCode::Char('n') => self.new_pane(),
            KeyCode::Char('p') => {
                self.set_show_worktrees_without_panes(!self.shows_worktrees_without_panes());
                Action::Consumed
            }
            KeyCode::Char('r') => Action::Reload,
            KeyCode::Char('!') => self.show_conditions(),
            KeyCode::Char('b') => self.set_state_filter(Some(StateFilter::Blocked)),
            KeyCode::Char('w') => self.set_state_filter(Some(StateFilter::Working)),
            KeyCode::Char('i') => self.set_state_filter(Some(StateFilter::Idle)),
            KeyCode::Char('d') => self.set_state_filter(Some(StateFilter::Done)),
            KeyCode::Char('a') => self.set_state_filter(None),
            KeyCode::Char('/') => {
                self.filtering = true;
                Action::Consumed
            }
            KeyCode::Char('D') => self.ask_to_remove(),
            // `Shift-S` beside `Shift-D`: the two keys that delete things are both shifted.
            KeyCode::Char('S') => self.set_sweeping(true),
            // A preselection, not a requirement: the branches picker asks which repository
            // anyway.
            KeyCode::Tab => Action::ShowBranches {
                repo_root: self
                    .selected_repo_index()
                    .map(|index| self.tree.repos[index].repo_root.clone()),
            },
            _ => Action::Ignored,
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            // Esc abandons the search; Enter keeps the filter and returns to commands.
            KeyCode::Esc => {
                self.filtering = false;
                self.options.query.clear();
                self.rebuild(None);
                Action::Consumed
            }
            KeyCode::Enter => {
                self.filtering = false;
                Action::Consumed
            }
            KeyCode::Backspace => {
                self.options.query.pop();
                self.rebuild(None);
                Action::Consumed
            }
            KeyCode::Down => {
                self.move_cursor(1);
                Action::Consumed
            }
            KeyCode::Up => {
                self.move_cursor(-1);
                Action::Consumed
            }
            // Arrows are not text, so they keep working while the search box has focus.
            KeyCode::Right => {
                self.move_group(1);
                Action::Consumed
            }
            KeyCode::Left => {
                self.move_group(-1);
                Action::Consumed
            }
            KeyCode::Char(c) => {
                self.options.query.push(c);
                self.rebuild(None);
                Action::Consumed
            }
            _ => Action::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {

    /// The key a checkout of the fixture's one repository is judged under.
    fn at(state: &PanesState, path: &str) -> (RepoKey, CheckoutPath) {
        (
            RepoKey::of(&state.tree.repos[0]),
            CheckoutPath::for_test(path),
        )
    }

    /// The answers map, spelled out per checkout: these tests care which of the four shapes
    /// a checkout is in.
    fn answers(pairs: &[(&str, WorkingTree)]) -> BTreeMap<CheckoutPath, WorkingTree> {
        pairs
            .iter()
            .map(|(path, answer)| (CheckoutPath::for_test(path), *answer))
            .collect()
    }
    use super::*;
    use crate::domain::model::{PaneNode, Refs, RepoNode, WorktreeNode};
    use crate::domain::sweep::{Half, Reason, Refusal};
    use crate::port::AgentStatus;
    use crate::port::{PullRequestOutcome, SettledPullRequest, Track};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn pane(id: &str, name: &str, status: AgentStatus) -> PaneNode {
        let workspace = id.split(':').next().unwrap().to_string();
        PaneNode {
            pane_id: id.into(),
            tab_id: format!("{workspace}:t1"),
            workspace_id: workspace,
            display_name: Some(name.into()),
            agent_status: status,
            focused: false,
        }
    }

    /// `me/app` with main (a working agent), feat/login (blocked), and an idle fix/crash.
    fn state() -> PanesState {
        PanesState::new(
            Tree {
                repos: vec![RepoNode {
                    repo_key: "/src/app/.git".into(),
                    repo_root: "/src/app".into(),
                    display_name: "me/app".into(),
                    refs: Refs::Read,
                    worktrees: vec![
                        WorktreeNode {
                            branch: Some("main".into()),
                            checkout_path: "/src/app".into(),
                            is_primary: true,
                            open_workspace_id: Some("w1".into()),
                            track: None,
                            panes: vec![pane("w1:p1", "claude", AgentStatus::Working)],
                        },
                        WorktreeNode {
                            branch: Some("feat/login".into()),
                            checkout_path: "/wt/app/feat-login".into(),
                            is_primary: false,
                            open_workspace_id: Some("w2".into()),
                            track: None,
                            panes: vec![pane("w2:p1", "codex", AgentStatus::Blocked)],
                        },
                        WorktreeNode {
                            branch: Some("fix/crash".into()),
                            checkout_path: "/wt/app/fix-crash".into(),
                            is_primary: false,
                            open_workspace_id: None,
                            track: None,
                            panes: vec![],
                        },
                    ],
                }],
                ungrouped: vec![pane("w9:p1", "zsh", AgentStatus::Unknown)],
            },
            None,
        )
    }

    /// Put the cursor on the row with this label.
    fn select(state: &mut PanesState, label: &str) {
        let index = state
            .rows()
            .iter()
            .position(|row| row.label == label)
            .unwrap_or_else(|| panic!("no row labelled {label}"));
        state.cursor = state
            .lines()
            .iter()
            .position(|line| *line == DisplayLine::Row(index))
            .unwrap();
    }

    fn cursor_label(state: &PanesState) -> String {
        match state.lines()[state.cursor] {
            DisplayLine::Row(index) => state.rows()[index].label.clone(),
            DisplayLine::Spacer => panic!("the cursor is on a spacer"),
        }
    }

    fn row_labels(state: &PanesState) -> Vec<String> {
        state.rows().iter().map(|r| r.label.clone()).collect()
    }

    /// A sweep with git's answers in place, and one checkout in each shape the sweep can
    /// see: `main` is the repository's own, `feat/login` is clean and nobody is finished
    /// with it, `fix/crash` has a gone upstream, and `feat/wip` has an agent in it.
    ///
    /// `feat/login`'s pane goes: a checkout with panes in it is refused before anything else
    /// is asked about it, which would leave the interesting half of these tests unreachable.
    fn sweeping() -> PanesState {
        let mut state = state();
        state.tree.repos[0].worktrees[1].panes.clear();
        state.tree.repos[0].worktrees[1].open_workspace_id = None;
        state.tree.repos[0].worktrees[2].track = Some(Track::Gone);
        state.tree.repos[0].worktrees.push(WorktreeNode {
            branch: Some("feat/wip".into()),
            checkout_path: "/wt/app/feat-wip".into(),
            is_primary: false,
            open_workspace_id: Some("w3".into()),
            track: Some(Track::Gone),
            panes: vec![pane("w3:p1", "codex", AgentStatus::Working)],
        });
        state.replace_tree(state.tree.clone());
        state.set_working_trees(answers(&[
            ("/src/app", WorkingTree::Clean),
            ("/wt/app/feat-login", WorkingTree::Clean),
            ("/wt/app/fix-crash", WorkingTree::Clean),
            ("/wt/app/feat-wip", WorkingTree::Clean),
        ]));
        assert_eq!(state.handle_key(key(KeyCode::Char('S'))), Action::Consumed);
        state
    }

    fn mark_of(state: &PanesState, label: &str) -> Option<Mark> {
        state
            .rows()
            .iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| panic!("no row labelled {label}"))
            .sweep
            .clone()
    }

    #[test]
    fn the_sweep_opens_with_what_git_already_knew_marked() {
        let state = sweeping();
        assert!(state.is_sweeping());
        assert_eq!(
            mark_of(&state, "fix/crash"),
            Some(Mark::Going(Reason::Gone)),
            "clean, nothing running in it, and its upstream is gone"
        );
        assert_eq!(mark_of(&state, "feat/login"), Some(Mark::Staying));
        assert_eq!(
            mark_of(&state, "main"),
            Some(Mark::Refused(Refusal::Primary))
        );
        assert_eq!(
            mark_of(&state, "feat/wip"),
            Some(Mark::Refused(Refusal::Running)),
            "its upstream is gone too, and somebody is working in it"
        );
        assert_eq!(
            mark_of(&state, "claude"),
            None,
            "a pane is not a checkout and has nothing to sweep"
        );
        assert_eq!(state.chosen(), vec![at(&state, "/wt/app/fix-crash")]);
    }

    #[test]
    fn a_working_tree_answering_clean_during_a_sweep_reaches_the_marks() {
        // `Clean` is the answer the sweep turns into a mark and the one answer no row draws,
        // so a rebuild keyed on the drawn markers never fires for it — and during a sweep
        // `/`, `r`, `b` and `d` are `Ignored`, so no other rebuild is in reach.
        let mut state = state();
        state.tree.repos[0].worktrees[1].panes.clear();
        state.tree.repos[0].worktrees[1].open_workspace_id = None;
        state.tree.repos[0].worktrees[2].track = Some(Track::Gone);
        state.replace_tree(state.tree.clone());
        state.handle_key(key(KeyCode::Char('S')));
        assert_eq!(
            state.marked_count(),
            0,
            "nobody has answered for anything yet"
        );

        state.set_working_trees(answers(&[("/wt/app/fix-crash", WorkingTree::Clean)]));
        assert_eq!(
            mark_of(&state, "fix/crash"),
            Some(Mark::Going(Reason::Gone)),
            "clean is what the sweep was waiting for"
        );
        assert_eq!(state.marked_count(), 1);
    }

    #[test]
    fn space_adds_a_mark_and_takes_it_away_again() {
        let mut state = sweeping();
        select(&mut state, "feat/login");

        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(mark_of(&state, "feat/login"), Some(Mark::GoingByHand));
        assert_eq!(
            state.chosen(),
            vec![
                at(&state, "/wt/app/feat-login"),
                at(&state, "/wt/app/fix-crash")
            ]
        );

        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(mark_of(&state, "feat/login"), Some(Mark::Staying));
        assert_eq!(state.chosen(), vec![at(&state, "/wt/app/fix-crash")]);
    }

    #[test]
    fn space_on_a_checkout_the_sweep_refuses_says_why_and_changes_nothing() {
        // The repository's own checkout, which `git worktree remove` will not take.
        let mut state = sweeping();
        select(&mut state, "main");

        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(state.message(), Some("the repository itself"));
        assert_eq!(
            mark_of(&state, "main"),
            Some(Mark::Refused(Refusal::Primary))
        );
        assert!(!state.chosen().contains(&at(&state, "/src/app")));
    }

    #[test]
    fn leaving_the_sweep_forgets_what_was_marked_in_it() {
        // ADR 0011: nothing is deleted that was not on the screen with a mark against it.
        let mut state = sweeping();
        select(&mut state, "feat/login");
        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(state.chosen().len(), 2);

        assert_eq!(state.handle_key(key(KeyCode::Esc)), Action::Consumed);
        assert!(!state.is_sweeping());
        assert!(
            state.chosen().is_empty(),
            "and nothing is marked outside one"
        );
        assert_eq!(mark_of(&state, "fix/crash"), None);

        state.handle_key(key(KeyCode::Char('S')));
        assert_eq!(
            state.chosen(),
            vec![at(&state, "/wt/app/fix-crash")],
            "the next sweep opens on what it suggests, not on what the last was talked into"
        );
    }

    #[test]
    fn a_sweep_judges_the_whole_list_because_it_opens_the_whole_list() {
        // `judge` walks the tree while the rows walk the filtered list, so a sweep entered
        // under a filter can mark checkouts nothing on screen mentions — ADR 0011.
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Esc));
        state.handle_key(key(KeyCode::Char('/')));
        for character in "login".chars() {
            state.handle_key(key(KeyCode::Char(character)));
        }
        state.handle_key(key(KeyCode::Enter));
        assert!(
            !row_labels(&state).contains(&"fix/crash".to_string()),
            "the filter is on and fix/crash is off screen"
        );

        state.handle_key(key(KeyCode::Char('S')));
        assert_eq!(state.query(), "", "the sweep opens the list it is judging");
        assert!(row_labels(&state).contains(&"fix/crash".to_string()));
        assert_eq!(
            state.chosen(),
            vec![at(&state, "/wt/app/fix-crash")],
            "and what is marked is on the screen it was marked on"
        );
    }

    #[test]
    fn a_state_filter_is_opened_by_a_sweep_too() {
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Esc));
        state.handle_key(key(KeyCode::Char('w')));
        assert!(state.state_filter().is_some());

        state.handle_key(key(KeyCode::Char('S')));
        assert!(state.state_filter().is_none());
        assert!(row_labels(&state).contains(&"fix/crash".to_string()));
    }

    #[test]
    fn the_cursor_stops_on_every_checkout_a_sweep_has_something_to_say_about() {
        // Outside a sweep the cursor steps over a checkout with panes in it. In a sweep the
        // checkout is the subject, so a cursor that cannot reach it makes its refusal
        // unreachable text.
        let mut state = sweeping();
        let mut reached = Vec::new();
        for _ in 0..state.lines().len() {
            if let Some(row) = state.rows().get(match state.lines()[state.cursor] {
                DisplayLine::Row(index) => index,
                DisplayLine::Spacer => continue,
            }) {
                reached.push(row.label.clone());
            }
            state.handle_key(key(KeyCode::Char('j')));
        }
        for checkout in ["main", "feat/login", "fix/crash", "feat/wip"] {
            assert!(
                reached.contains(&checkout.to_string()),
                "{checkout} is a checkout the sweep has an answer for, and `j` never reached it"
            );
        }

        // And the refusal it was stepping over is now askable.
        select(&mut state, "feat/wip");
        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(state.message(), Some("panes are running in it"));
    }

    #[test]
    fn q_out_of_a_sweep_puts_the_picker_back_rather_than_closing_it() {
        let mut state = sweeping();
        assert_eq!(state.handle_key(key(KeyCode::Char('q'))), Action::Consumed);
        assert!(!state.is_sweeping());
        assert_eq!(
            state.handle_key(key(KeyCode::Char('q'))),
            Action::Quit,
            "and the second one closes it"
        );
    }

    #[test]
    fn every_way_out_of_a_sweep_is_a_way_out_of_a_sweep() {
        // The footer offers `shift+s done` beside `esc done`, so falling through to
        // `Ignored` is a picker that looks frozen on the key it just told you to press.
        for out in [KeyCode::Char('S'), KeyCode::Esc, KeyCode::Char('q')] {
            let mut state = sweeping();
            select(&mut state, "feat/login");
            state.handle_key(key(KeyCode::Char(' ')));
            assert_eq!(state.chosen().len(), 2);

            assert_eq!(state.handle_key(key(out)), Action::Consumed, "{out:?}");
            assert!(!state.is_sweeping(), "{out:?} did not leave the sweep");
            assert!(state.chosen().is_empty(), "{out:?} kept the marks");
        }
    }

    #[test]
    fn entering_a_sweep_from_a_search_leaves_the_cursor_on_what_was_searched_for() {
        // The sweep opens the whole list, so a line index from the filtered list points at a
        // different checkout. On a pane row `Space` is answered and says nothing, so the key
        // the user reaches for next does nothing at all.
        for query in ["crash", "wip", "login"] {
            let mut state = sweeping();
            state.handle_key(key(KeyCode::Esc));
            state.handle_key(key(KeyCode::Char('/')));
            for character in query.chars() {
                state.handle_key(key(KeyCode::Char(character)));
            }
            state.handle_key(key(KeyCode::Enter));
            let found = cursor_label(&state);

            state.handle_key(key(KeyCode::Char('S')));
            assert_eq!(
                cursor_label(&state),
                found,
                "/{query} then Shift-S moved the cursor off the row it was on"
            );
        }

        // And the row it stays on is the one that was searched for, not merely some row.
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Esc));
        state.handle_key(key(KeyCode::Char('/')));
        for character in "crash".chars() {
            state.handle_key(key(KeyCode::Char(character)));
        }
        state.handle_key(key(KeyCode::Enter));
        state.handle_key(key(KeyCode::Char('S')));
        assert_eq!(cursor_label(&state), "fix/crash");
        assert_eq!(row_labels(&state).len(), 9, "on the whole list");
    }

    #[test]
    fn entering_a_sweep_leaves_the_cursor_where_it_was() {
        // Nothing about the list changes except what each row says about itself, and where
        // the cursor is is the whole of what the user is deciding about.
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Esc));
        select(&mut state, "fix/crash");
        let at = state.cursor;

        state.handle_key(key(KeyCode::Char('S')));
        assert_eq!(state.cursor, at);
        assert_eq!(cursor_label(&state), "fix/crash");
    }

    #[test]
    fn a_removal_running_under_a_sweep_takes_the_checkout_out_of_it() {
        // Offering to delete a checkout that is already being deleted is the one refusal
        // about something happening right now rather than about what the checkout is.
        let mut state = sweeping();
        assert!(state.chosen().contains(&at(&state, "/wt/app/fix-crash")));

        state.set_removing(vec![CheckoutPath::for_test("/wt/app/fix-crash")]);
        assert_eq!(
            mark_of(&state, "fix/crash"),
            Some(Mark::Refused(Refusal::Removing))
        );
        assert!(!state.chosen().contains(&at(&state, "/wt/app/fix-crash")));

        // And when it ends without removing anything — git refused it, say — the row goes
        // back to being the sweep's to offer.
        state.set_removing(Vec::new());
        assert_eq!(
            mark_of(&state, "fix/crash"),
            Some(Mark::Going(Reason::Gone))
        );
    }

    #[test]
    fn a_mark_does_not_move_to_whatever_is_at_that_path_next() {
        // A removal finishing re-reads the tree, and a second herdr session can have made a
        // worktree where the last one was.
        let mut state = sweeping();
        select(&mut state, "feat/login");
        state.handle_key(key(KeyCode::Char(' ')));
        assert!(state.chosen().contains(&at(&state, "/wt/app/feat-login")));

        let mut moved = state.tree.clone();
        moved.repos[0].worktrees[1].branch = Some("release/v2".into());
        state.replace_tree(moved);

        assert!(
            !state.chosen().contains(&at(&state, "/wt/app/feat-login")),
            "release/v2 has never been on the screen with a mark against it"
        );
        assert_eq!(mark_of(&state, "release/v2"), Some(Mark::Staying));
    }

    #[test]
    fn a_tree_read_again_under_a_sweep_keeps_the_cursor_on_the_checkout_it_was_on() {
        // Put back by line index against a list one row shorter, the cursor lands on the
        // next checkout down, and the `Space` the user had lined up marks a checkout they
        // never pointed at. That is the list `Enter` deletes.
        let mut state = sweeping();
        let mut tree = state.tree().clone();
        tree.repos[0].worktrees[3].panes.clear();
        tree.repos[0].worktrees[3].open_workspace_id = None;
        state.replace_tree(tree);
        select(&mut state, "fix/crash");

        let mut shorter = state.tree().clone();
        shorter.repos[0].worktrees.remove(1);
        state.replace_tree(shorter);

        assert_eq!(cursor_label(&state), "fix/crash");
        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(
            mark_of(&state, "fix/crash"),
            Some(Mark::Staying),
            "the Space landed on the row the user was looking at"
        );
        assert!(
            state.chosen().contains(&at(&state, "/wt/app/feat-wip")),
            "and not on the one below it"
        );
    }

    #[test]
    fn leaving_a_sweep_takes_the_cursor_off_a_checkout_it_may_no_longer_stop_on() {
        // Inside a sweep the cursor stops on a checkout with panes in it; outside it does
        // not, so a cursor put back there by name sits where the arrow keys can never take
        // it again.
        let mut state = sweeping();
        select(&mut state, "feat/wip");
        state.handle_key(key(KeyCode::Esc));

        assert!(!state.is_sweeping());
        let row = state.selected().expect("the cursor is on a row");
        assert!(row.is_selectable(), "on a row it may stop on");
        assert_eq!(
            row.label, "codex",
            "the pane under the checkout, which is where to go"
        );
    }

    #[test]
    fn a_tree_read_again_under_a_sweep_is_judged_again() {
        let mut state = sweeping();
        assert!(state.chosen().contains(&at(&state, "/wt/app/fix-crash")));

        // The upstream came back — somebody pushed the branch again.
        let mut tree = state.tree.clone();
        tree.repos[0].worktrees[2].track = None;
        state.replace_tree(tree);
        assert_eq!(mark_of(&state, "fix/crash"), Some(Mark::Staying));
        assert!(state.chosen().is_empty());
    }

    #[test]
    fn space_on_a_row_that_is_not_a_checkout_is_answered_and_says_nothing() {
        // A repository heading, or a pane. `Ignored` would fall through to the loop as an
        // unhandled key, and a message would be noise on every stray press.
        let mut state = sweeping();
        select(&mut state, "claude");
        assert_eq!(state.handle_key(key(KeyCode::Char(' '))), Action::Consumed);
        assert_eq!(state.message(), None);
        assert_eq!(state.chosen(), vec![at(&state, "/wt/app/fix-crash")]);
    }

    #[test]
    fn the_cursor_keys_go_the_way_they_are_drawn_during_a_sweep() {
        // `j` down and `k` up. Asserting only that the cursor moved and came back is
        // satisfied by a pair that both go the wrong way.
        let mut state = sweeping();
        select(&mut state, "main");
        state.handle_key(key(KeyCode::Char('j')));
        assert_eq!(cursor_label(&state), "claude", "j is down");
        state.handle_key(key(KeyCode::Char('k')));
        assert_eq!(cursor_label(&state), "main", "and k is back up");
    }

    #[test]
    fn the_keys_that_act_on_one_row_are_not_answered_during_a_sweep() {
        for code in [
            KeyCode::Char('D'),
            KeyCode::Tab,
            KeyCode::Char('r'),
            KeyCode::Char('n'),
            KeyCode::Char('/'),
        ] {
            let mut state = sweeping();
            select(&mut state, "fix/crash");
            assert_eq!(
                state.handle_key(key(code)),
                Action::Ignored,
                "{code:?} means something outside a sweep and nothing in one"
            );
            assert!(state.is_sweeping(), "and it did not leave the sweep either");
        }
    }

    #[test]
    fn the_cursor_still_walks_the_list_during_a_sweep() {
        let mut state = sweeping();
        let first = state.cursor;
        assert_eq!(state.handle_key(key(KeyCode::Char('j'))), Action::Consumed);
        assert_ne!(state.cursor, first);
        assert_eq!(state.handle_key(key(KeyCode::Char('k'))), Action::Consumed);
        assert_eq!(state.cursor, first);
    }

    #[test]
    fn a_row_gh_could_not_judge_says_so_and_is_still_the_users_to_mark() {
        let mut state = sweeping();
        state.set_settled(
            BTreeMap::from([(RepoRoot::of(&state.tree.repos[0]), None)]),
            Some("gh could not be run: no such file or directory".to_string()),
            false,
        );

        assert_eq!(
            mark_of(&state, "feat/login"),
            Some(Mark::Unjudged(Half::PullRequests))
        );
        assert_eq!(
            mark_of(&state, "fix/crash"),
            Some(Mark::Going(Reason::Gone)),
            "gh may only widen a sweep: it never clears a mark git put there"
        );
        assert_eq!(
            state.sweep_trouble(),
            Some("gh could not be run: no such file or directory")
        );

        select(&mut state, "feat/login");
        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(
            mark_of(&state, "feat/login"),
            Some(Mark::GoingUnjudged(Half::PullRequests)),
            "not judged is not refused — and the row goes on saying it was not judged"
        );
    }

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
    fn a_pull_request_gh_found_widens_the_sweep_and_says_which_one() {
        let mut state = sweeping();
        state.set_settled(
            BTreeMap::from([(
                RepoRoot::of(&state.tree.repos[0]),
                Some(SettledPullRequests::All(vec![SettledPullRequest {
                    number: 42,
                    head_ref: "feat/login".to_string(),
                    from_a_fork: false,
                    outcome: PullRequestOutcome::Merged,
                }])),
            )]),
            None,
            false,
        );

        assert_eq!(
            mark_of(&state, "feat/login"),
            Some(Mark::Going(Reason::PullRequest {
                number: 42,
                outcome: PullRequestOutcome::Merged,
            }))
        );
        assert_eq!(state.sweep_trouble(), None);
        assert_eq!(
            state.chosen(),
            vec![
                at(&state, "/wt/app/feat-login"),
                at(&state, "/wt/app/fix-crash")
            ]
        );
    }

    #[test]
    fn gh_landing_does_not_move_the_cursor() {
        // It arrives on its own, after a frame the user is already reading: a cursor that
        // jumped would move the row under a `Space` about to be pressed.
        let mut state = sweeping();
        select(&mut state, "feat/login");
        let at = state.cursor;
        state.set_settled(
            BTreeMap::from([(RepoRoot::of(&state.tree.repos[0]), None)]),
            None,
            false,
        );
        assert_eq!(state.cursor, at);
    }

    #[test]
    fn a_sweep_says_it_was_entered_once_and_then_stops_saying_so() {
        // The loop asks on every frame and answers a yes by asking `gh` again where it
        // refused, so a second yes is a `gh` call per frame for as long as it keeps
        // refusing.
        let mut state = state();
        assert!(!state.sweep_entered(), "no sweep, so nothing was entered");
        state.handle_key(key(KeyCode::Char('S')));
        assert!(state.sweep_entered());
        assert!(!state.sweep_entered(), "once per entry");
        state.handle_key(key(KeyCode::Esc));
        assert!(!state.sweep_entered(), "leaving is not entering");
        state.handle_key(key(KeyCode::Char('S')));
        assert!(state.sweep_entered(), "and once more on the next entry");
    }

    #[test]
    fn git_answering_does_not_move_the_cursor_either() {
        // The working trees report in one at a time in the seconds after the picker opens.
        // Relisted from the top on each, the cursor is yanked back once per answer, with
        // `Shift-D` aimed at whatever is then beneath it.
        let mut state = state();
        select(&mut state, "fix/crash");
        let at = state.cursor;
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Dirty)]));
        assert_eq!(state.cursor, at);
        assert_eq!(cursor_label(&state), "fix/crash");
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
    fn what_gh_says_outside_a_sweep_has_nowhere_to_go() {
        let mut state = state();
        state.set_settled(
            BTreeMap::from([(RepoRoot::of(&state.tree.repos[0]), None)]),
            Some("gh could not be run".to_string()),
            false,
        );
        assert_eq!(state.sweep_trouble(), None);
        assert!(state.chosen().is_empty());
    }

    #[test]
    fn enter_on_a_pane_jumps_to_it() {
        let mut state = state();
        select(&mut state, "codex");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Action::Jump("w2:p1".into())
        );
    }

    #[test]
    fn enter_on_a_worktree_that_is_already_open_goes_to_its_work() {
        // The cursor does not stop here; the panes listed under it are what you pick.
        let mut state = state();
        select(&mut state, "main");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Action::Jump("w1:p1".into())
        );
    }

    #[test]
    fn enter_on_a_worktree_with_no_pane_opens_the_checkout() {
        let mut state = state();
        select(&mut state, "fix/crash");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Action::OpenWorktree {
                repo_root: "/src/app".into(),
                checkout_path: "/wt/app/fix-crash".into(),
            }
        );
    }

    #[test]
    fn the_cursor_visits_only_panes_and_checkouts_with_nothing_running() {
        let mut state = state();
        let first = state.selected().unwrap().label.clone();
        let mut stops = vec![first.clone()];
        for _ in 0..state.lines().len() {
            state.handle_key(key(KeyCode::Down));
            let label = state.selected().unwrap().label.clone();
            if label == first {
                break;
            }
            stops.push(label);
        }
        assert_eq!(stops, ["claude", "codex", "fix/crash", "zsh"]);

        // And the rows it stepped over are still on screen.
        assert_eq!(
            row_labels(&state),
            [
                "me/app (2)",
                "main",
                "claude",
                "feat/login",
                "codex",
                "fix/crash",
                "not in any repository (1)",
                "zsh",
            ]
        );
    }

    #[test]
    fn the_arrows_move_between_repositories() {
        // The fixture has one repository plus the panes in none of them.
        let mut state = state();
        assert_eq!(state.selected().unwrap().label, "claude");

        state.handle_key(key(KeyCode::Right));
        assert_eq!(
            state.selected().unwrap().label,
            "zsh",
            "the panes in no repository are a section like any other"
        );
        state.handle_key(key(KeyCode::Right));
        assert_eq!(state.selected().unwrap().label, "claude", "and it wraps");

        // From deep inside a group, one press still leaves it.
        state.handle_key(key(KeyCode::Down));
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected().unwrap().label, "fix/crash");
        state.handle_key(key(KeyCode::Left));
        assert_eq!(state.selected().unwrap().label, "zsh");
    }

    #[test]
    fn h_and_l_are_the_arrows_by_another_name() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('l')));
        assert_eq!(state.selected().unwrap().label, "zsh");
        state.handle_key(key(KeyCode::Char('h')));
        assert_eq!(state.selected().unwrap().label, "claude");
    }

    #[test]
    fn the_arrows_keep_working_while_the_search_box_has_focus() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        assert!(state.is_filtering());
        state.handle_key(key(KeyCode::Right));
        assert_eq!(state.selected().unwrap().label, "zsh");
        assert_eq!(state.query(), "", "an arrow is not text");

        // A letter is, though: `l` types rather than moves once the search box has focus.
        state.handle_key(key(KeyCode::Char('l')));
        assert_eq!(state.query(), "l");
    }

    #[test]
    fn moving_up_from_the_top_wraps_to_the_last_thing_worth_going_to() {
        let mut state = state();
        state.handle_key(key(KeyCode::Up));
        assert_eq!(state.selected().unwrap().label, "zsh");
    }

    #[test]
    fn n_adds_a_pane_beside_the_work_already_in_that_checkout() {
        let mut state = state();
        select(&mut state, "codex");
        assert_eq!(
            state.handle_key(key(KeyCode::Char('n'))),
            Action::NewPane {
                checkout_path: "/wt/app/feat-login".into(),
                beside_pane_id: "w2:p1".into(),
            }
        );
    }

    #[test]
    fn n_on_a_checkout_with_no_pane_opens_it_since_there_is_nothing_to_split() {
        let mut state = state();
        select(&mut state, "fix/crash");
        assert!(matches!(
            state.handle_key(key(KeyCode::Char('n'))),
            Action::OpenWorktree { .. }
        ));
    }

    #[test]
    fn shift_d_asks_before_it_deletes_anything() {
        let mut state = state();
        select(&mut state, "fix/crash");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);

        let asked = state.pending_removal().expect("a question should be up");
        assert_eq!(asked.label(), "fix/crash");
        assert_eq!(asked.checkout_path().as_str(), "/wt/app/fix-crash");
        assert_eq!(asked.repo_root(), "/src/app");

        let Action::RemoveWorktree(asked) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        assert_eq!(asked.repo_root(), "/src/app");
        assert_eq!(asked.checkout_path().as_str(), "/wt/app/fix-crash");
        assert_eq!(asked.label(), "fix/crash");
        assert!(asked.panes().is_empty(), "there were none to close");
        assert!(!asked.delete_branch(), "Shift-D keeps the branch");
        assert!(
            state.pending_removal().is_none(),
            "the question is answered"
        );
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
    fn anything_that_is_not_y_is_a_no() {
        // Including keys nobody thought of.
        for code in [
            KeyCode::Char('n'),
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Char('D'),
            KeyCode::Down,
            KeyCode::Char('Y'),
        ] {
            let mut state = state();
            select(&mut state, "fix/crash");
            state.handle_key(key(KeyCode::Char('D')));
            assert_eq!(state.handle_key(key(code)), Action::Consumed, "{code:?}");
            assert!(state.pending_removal().is_none(), "{code:?}");
        }
    }

    #[test]
    fn shift_d_on_a_pane_offers_to_delete_the_checkout_it_is_in() {
        let mut state = state();
        // Two panes, so the assertion below can tell "the checkout's panes" from "the first
        // pane in the checkout", and can see them reordered.
        let mut tree = state.tree().clone();
        tree.repos[0].worktrees[1]
            .panes
            .push(pane("w2:p2", "zsh", AgentStatus::Unknown));
        state.replace_tree(tree);
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Clean)]));
        select(&mut state, "codex");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);

        let asked = state.pending_removal().expect("a question should be up");
        assert_eq!(asked.label(), "feat/login");
        let closing: Vec<&str> = asked.panes().iter().map(|p| p.pane_id.as_str()).collect();
        assert_eq!(closing, ["w2:p1", "w2:p2"], "and it names what stops");

        // And `y` carries them through in the order the question listed them: otherwise the
        // picker asks about panes it never closes.
        let Action::RemoveWorktree(asked) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        assert_eq!(asked.checkout_path().as_str(), "/wt/app/feat-login");
        let closing: Vec<&str> = asked.panes().iter().map(|p| p.pane_id.as_str()).collect();
        assert_eq!(closing, ["w2:p1", "w2:p2"]);
    }

    #[test]
    fn a_checkout_with_panes_whose_working_tree_has_not_answered_yet_is_refused() {
        // The state the picker opens in: nothing has answered yet, and the cursor is already
        // on the pane the user came from. `r` puts every checkout back into it, so this is
        // not only a startup window.
        let mut state = state();
        select(&mut state, "codex");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("still reading that working tree — try again")
        );

        // An empty checkout is offered either way: nothing is at stake before the question,
        // and git answers for itself.
        select(&mut state, "fix/crash");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(state.pending_removal().is_some());
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
    fn a_checkout_already_being_removed_is_not_offered_again() {
        // The checkout's row stops being selectable but its panes' rows do not, so a second
        // confirmation is still reachable from a pane.
        let mut state = state();
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Clean)]));
        state.set_removing(vec![CheckoutPath::for_test("/wt/app/feat-login")]);
        select(&mut state, "codex");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("that checkout is already being removed")
        );
    }

    #[test]
    fn a_checkout_with_panes_is_refused_before_the_question_when_it_is_holding_work() {
        let mut state = state();
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Dirty)]));
        select(&mut state, "codex");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("that checkout is holding work nobody has committed")
        );
    }

    #[test]
    fn a_checkout_with_panes_is_refused_when_git_would_not_read_it() {
        // Which is not the same as reading it and finding nothing: without an answer there
        // is no protection to offer, and the panes would go on the strength of a guess.
        let mut state = state();
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Unreadable)]));
        select(&mut state, "codex");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(state.pending_removal().is_none());
        assert_eq!(
            state.message(),
            Some("git would not read that working tree")
        );
    }

    #[test]
    fn an_empty_checkout_holding_work_is_still_offered_and_left_to_git() {
        // Nothing is at stake before the question here, and git's refusal is the answer
        // rather than an obstacle — it says what would have been lost.
        let mut state = state();
        state.set_working_trees(answers(&[("/wt/app/fix-crash", WorkingTree::Dirty)]));
        select(&mut state, "fix/crash");
        state.handle_key(key(KeyCode::Char('D')));
        assert!(state.pending_removal().is_some());
    }

    #[test]
    fn the_repositorys_own_checkout_is_never_offered() {
        let mut on_a_pane = state();
        select(&mut on_a_pane, "claude");
        assert_eq!(
            on_a_pane.handle_key(key(KeyCode::Char('D'))),
            Action::Consumed
        );
        assert!(on_a_pane.pending_removal().is_none());
        assert!(on_a_pane.message().is_some(), "and it says why");

        // Nothing running in it, so the cursor can reach the checkout's own row.
        let mut on_the_repo = PanesState::new(
            Tree {
                repos: vec![RepoNode {
                    repo_key: "/src/app/.git".into(),
                    repo_root: "/src/app".into(),
                    display_name: "me/app".into(),
                    refs: Refs::Read,
                    worktrees: vec![
                        WorktreeNode {
                            branch: Some("main".into()),
                            checkout_path: "/src/app".into(),
                            is_primary: true,
                            open_workspace_id: None,
                            track: None,
                            panes: vec![],
                        },
                        WorktreeNode {
                            branch: Some("feat/login".into()),
                            checkout_path: "/wt/app/feat-login".into(),
                            is_primary: false,
                            open_workspace_id: Some("w2".into()),
                            track: None,
                            panes: vec![pane("w2:p1", "codex", AgentStatus::Blocked)],
                        },
                    ],
                }],
                ungrouped: vec![],
            },
            None,
        );
        select(&mut on_the_repo, "main");
        on_the_repo.handle_key(key(KeyCode::Char('D')));
        assert!(on_the_repo.pending_removal().is_none());
        assert!(
            on_the_repo
                .message()
                .unwrap_or_default()
                .contains("the repository itself"),
            "got {:?}",
            on_the_repo.message()
        );
    }

    #[test]
    fn tab_asks_for_the_branches_of_the_repository_the_cursor_is_in() {
        let mut state = state();
        select(&mut state, "codex");
        assert_eq!(
            state.handle_key(key(KeyCode::Tab)),
            Action::ShowBranches {
                repo_root: Some("/src/app".into())
            }
        );
    }

    #[test]
    fn the_state_keys_narrow_to_one_agent_state_and_a_clears_them() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('b')));
        assert_eq!(state.state_filter(), Some(StateFilter::Blocked));
        assert_eq!(row_labels(&state), ["me/app (2)", "feat/login", "codex"]);

        state.handle_key(key(KeyCode::Char('w')));
        assert_eq!(state.state_filter(), Some(StateFilter::Working));
        assert_eq!(row_labels(&state), ["me/app (2)", "main", "claude"]);

        state.handle_key(key(KeyCode::Char('a')));
        assert_eq!(state.state_filter(), None);
        assert!(row_labels(&state).contains(&"fix/crash".to_string()));
    }

    #[test]
    fn pressing_the_same_state_key_twice_clears_it_so_it_is_never_a_one_way_door() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('b')));
        assert_eq!(state.state_filter(), Some(StateFilter::Blocked));
        state.handle_key(key(KeyCode::Char('b')));
        assert_eq!(state.state_filter(), None);
    }

    #[test]
    fn panes_that_are_not_in_a_repository_are_always_listed() {
        // They are still panes. A picker that hides some of them makes you wonder which.
        assert!(row_labels(&state()).contains(&"zsh".to_string()));
    }

    #[test]
    fn p_toggles_worktrees_without_panes() {
        let mut state = state();
        state.set_show_worktrees_without_panes(false);
        assert!(!row_labels(&state).contains(&"fix/crash".to_string()));

        assert_eq!(state.handle_key(key(KeyCode::Char('p'))), Action::Consumed);
        assert!(state.shows_worktrees_without_panes());
        assert!(row_labels(&state).contains(&"fix/crash".to_string()));

        state.handle_key(key(KeyCode::Char('p')));
        assert!(!state.shows_worktrees_without_panes());
        assert!(!row_labels(&state).contains(&"fix/crash".to_string()));
    }

    #[test]
    fn p_keeps_the_cursor_on_a_still_visible_pane() {
        let mut state = state();
        // This pane follows the row that disappears, so this also checks the cursor repair.
        select(&mut state, "zsh");

        assert_eq!(state.handle_key(key(KeyCode::Char('p'))), Action::Consumed);
        assert_eq!(cursor_label(&state), "zsh");
    }

    #[test]
    fn hide_moves_the_cursor_off_a_worktree_that_disappears() {
        let mut state = state();
        select(&mut state, "fix/crash");
        state.set_show_worktrees_without_panes(false);
        assert_ne!(cursor_label(&state), "fix/crash");
        assert!(rows::selectable(
            state.rows(),
            state.lines(),
            state.cursor()
        ));
    }

    #[test]
    fn a_sweep_temporarily_shows_worktrees_without_panes() {
        let mut state = state();
        state.set_show_worktrees_without_panes(false);
        assert!(!row_labels(&state).contains(&"fix/crash".to_string()));

        state.handle_key(key(KeyCode::Char('S')));
        assert!(row_labels(&state).contains(&"fix/crash".to_string()));

        state.handle_key(key(KeyCode::Esc));
        assert!(!row_labels(&state).contains(&"fix/crash".to_string()));
    }

    #[test]
    fn slash_starts_searching_and_typing_narrows_the_list() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        assert!(state.is_filtering());
        for c in "codex".chars() {
            state.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(state.query(), "codex");
        assert_eq!(row_labels(&state), ["me/app (2)", "feat/login", "codex"]);
    }

    #[test]
    fn while_searching_letters_are_text_rather_than_commands() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        // `b` would apply a state filter outside search mode.
        assert_eq!(state.handle_key(key(KeyCode::Char('b'))), Action::Consumed);
        assert_eq!(state.query(), "b");
        assert_eq!(state.state_filter(), None);
    }

    #[test]
    fn ctrl_u_clears_the_search_the_way_the_navigator_does() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        for c in "code".chars() {
            state.handle_key(key(KeyCode::Char(c)));
        }
        state.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(state.query(), "");
        assert!(state.is_filtering(), "still searching, just empty");
    }

    #[test]
    fn escape_abandons_the_search_but_enter_keeps_it() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('/')));
        state.handle_key(key(KeyCode::Char('c')));
        state.handle_key(key(KeyCode::Enter));
        assert!(!state.is_filtering());
        assert_eq!(state.query(), "c");

        state.handle_key(key(KeyCode::Char('/')));
        state.handle_key(key(KeyCode::Esc));
        assert!(!state.is_filtering());
        assert_eq!(state.query(), "");
    }

    #[test]
    fn escape_and_q_quit_when_not_searching() {
        assert_eq!(state().handle_key(key(KeyCode::Esc)), Action::Quit);
        assert_eq!(state().handle_key(key(KeyCode::Char('q'))), Action::Quit);
        assert_eq!(
            state().handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }

    #[test]
    fn moving_the_cursor_never_lands_on_a_blank_line() {
        let mut state = state();
        state.handle_key(key(KeyCode::Char('h')));
        for _ in 0..state.lines().len() * 2 {
            state.handle_key(key(KeyCode::Char('j')));
            assert!(
                matches!(state.lines()[state.cursor()], DisplayLine::Row(_)),
                "cursor landed on a blank line"
            );
        }
        for _ in 0..state.lines().len() * 2 {
            state.handle_key(key(KeyCode::Char('k')));
            assert!(matches!(state.lines()[state.cursor()], DisplayLine::Row(_)));
        }
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

    #[test]
    fn the_key_that_reads_what_is_wrong_answers_when_nothing_is() {
        // A panel with no lines in it is not an answer to a question the user asked with a
        // keypress, and a key that appears to do nothing is the same bug as a blank line.
        let mut state = state();
        assert!(state.conditions().is_empty());

        assert_eq!(state.handle_key(key(KeyCode::Char('!'))), Action::Consumed);
        assert!(!state.is_showing_conditions());
        assert_eq!(state.message(), Some("nothing is wrong"));
    }

    #[test]
    fn the_panel_opens_on_what_is_wrong_and_closes_three_ways() {
        for closing in [KeyCode::Char('!'), KeyCode::Esc, KeyCode::Char('q')] {
            let mut state = state();
            state.set_stale("herdr did not answer".into());

            assert_eq!(state.handle_key(key(KeyCode::Char('!'))), Action::Consumed);
            assert!(state.is_showing_conditions());
            // And `q` closes the panel rather than the picker, the way it leaves a sweep.
            assert_eq!(state.handle_key(key(closing)), Action::Consumed);
            assert!(!state.is_showing_conditions(), "{closing:?} closes it");
        }
    }

    #[test]
    fn a_key_the_panel_has_no_use_for_does_not_reach_the_list_under_it() {
        let mut state = state();
        state.set_stale("herdr did not answer".into());
        state.handle_key(key(KeyCode::Char('!')));
        let before = state.cursor;

        assert_eq!(state.handle_key(key(KeyCode::Char('j'))), Action::Consumed);
        assert_eq!(state.cursor, before, "the cursor is not where the user is");
        assert_eq!(state.handle_key(key(KeyCode::Char('D'))), Action::Consumed);
        assert!(state.pending_removal().is_none(), "and nothing is asked");
        assert!(state.is_showing_conditions(), "the panel is still up");

        // Abandoning the picker is the one key it does not swallow.
        let quit = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(state.handle_key(quit), Action::Quit);
    }

    #[test]
    fn a_sweeps_question_takes_the_panel_off_the_screen() {
        // Both are drawn over the list, and the one that can delete something owns the
        // space: a box nobody saw is a box whose `y` is armed over nothing.
        let mut state = sweeping();
        state.set_stale("herdr did not answer".into());
        // The order a user reaches this in: `Enter` asks for the reading the question waits
        // on, and the panel is opened while that reading is still out.
        state.handle_key(key(KeyCode::Enter));
        assert_eq!(state.handle_key(key(KeyCode::Char('!'))), Action::Consumed);
        assert!(state.is_showing_conditions(), "a sweep can read it too");

        state.set_waiting(false);
        state.confirm_sweep_if_settled();
        assert!(state.pending_sweep().is_some(), "the question is up");
        assert!(!state.is_showing_conditions());
    }

    #[test]
    fn a_key_release_is_ignored_so_windows_does_not_act_twice() {
        let mut state = state();
        let release =
            KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Release);
        assert_eq!(state.handle_key(release), Action::Ignored);
    }

    /// `Enter`, and the frame on which the walk it asked for has answered.
    fn ask(state: &mut PanesState) -> Action {
        let action = state.handle_key(key(KeyCode::Enter));
        state.set_waiting(false);
        state.confirm_sweep_if_settled();
        action
    }

    /// What the sweep's box lists, by label.
    fn box_labels(state: &PanesState) -> Vec<&str> {
        state
            .pending_sweep()
            .map(|sweep| sweep.removals().iter().map(|r| r.label()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn enter_with_nothing_marked_says_so_and_asks_for_no_re_read() {
        let mut state = sweeping();
        select(&mut state, "fix/crash");
        state.handle_key(key(KeyCode::Char(' ')));
        assert!(state.chosen().is_empty());

        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Consumed);
        assert_eq!(state.message(), Some("nothing is marked"));
        state.set_waiting(false);
        state.confirm_sweep_if_settled();
        assert!(state.pending_sweep().is_none());
    }

    #[test]
    fn enter_asks_for_a_re_read_before_it_asks_anything() {
        let mut state = sweeping();
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::SweepReload);
        assert!(
            state.pending_sweep().is_none(),
            "the box waits for the re-read"
        );
        assert!(state.is_sweeping());
    }

    #[test]
    fn the_question_waits_for_the_walk_to_finish() {
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Enter));
        state.set_waiting(true);
        state.confirm_sweep_if_settled();
        assert!(state.pending_sweep().is_none(), "the walk is still out");

        state.set_waiting(false);
        state.confirm_sweep_if_settled();
        assert_eq!(box_labels(&state), ["fix/crash"]);
    }

    #[test]
    fn a_row_the_re_read_added_is_in_the_box() {
        let mut state = state();
        state.tree.repos[0].worktrees[1].panes.clear();
        state.tree.repos[0].worktrees[1].open_workspace_id = None;
        state.tree.repos[0].worktrees[2].track = Some(Track::Gone);
        state.replace_tree(state.tree.clone());
        state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Clean)]));
        state.handle_key(key(KeyCode::Char('S')));
        select(&mut state, "feat/login");
        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(state.chosen(), vec![at(&state, "/wt/app/feat-login")]);

        state.handle_key(key(KeyCode::Enter));
        state.set_working_trees(answers(&[
            ("/wt/app/feat-login", WorkingTree::Clean),
            ("/wt/app/fix-crash", WorkingTree::Clean),
        ]));
        state.set_waiting(false);
        state.confirm_sweep_if_settled();

        assert_eq!(box_labels(&state), ["feat/login", "fix/crash"]);
        assert_eq!(
            state.message(),
            None,
            "nothing was dropped, so nothing is said"
        );
    }

    #[test]
    fn y_removes_exactly_what_the_box_listed_and_leaves_the_sweep() {
        let mut state = sweeping();
        ask(&mut state);
        assert_eq!(box_labels(&state), ["fix/crash"]);

        let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
            panic!("`y` is the answer that goes ahead");
        };
        let paths: Vec<&str> = sweep
            .removals()
            .iter()
            .map(|r| r.checkout_path().as_str())
            .collect();
        assert_eq!(paths, ["/wt/app/fix-crash"]);
        assert!(
            !state.is_sweeping(),
            "the sweep is over once it is answered"
        );
        assert!(state.pending_sweep().is_none());
        assert!(state.chosen().is_empty(), "and the marks went with it");
    }

    #[test]
    fn any_other_key_takes_the_sweeps_question_back() {
        for code in [
            KeyCode::Char('n'),
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Char('D'),
            KeyCode::Down,
            KeyCode::Char('Y'),
            KeyCode::Char(' '),
        ] {
            let mut state = sweeping();
            ask(&mut state);
            assert_eq!(state.handle_key(key(code)), Action::Consumed, "{code:?}");
            assert!(state.pending_sweep().is_none(), "{code:?}");
            assert!(
                state.is_sweeping(),
                "{code:?}: the sweep stays; its question went"
            );
            assert_eq!(
                state.chosen(),
                vec![at(&state, "/wt/app/fix-crash")],
                "{code:?}: and the marks stay with it"
            );
        }
    }

    #[test]
    fn a_tree_that_changes_under_the_sweeps_question_takes_it_back() {
        let mut state = sweeping();
        ask(&mut state);
        assert!(state.pending_sweep().is_some());

        state.replace_tree(state.tree.clone());
        assert!(state.pending_sweep().is_none());
        assert_eq!(
            state.message(),
            Some("the list changed while that was up — ask again")
        );
        assert_eq!(state.handle_key(key(KeyCode::Char('y'))), Action::Ignored);

        // The re-read `Enter` asks for replaces the tree too, and that is not a change under
        // the question: it comes before it.
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Enter));
        state.replace_tree(state.tree.clone());
        state.set_waiting(false);
        state.confirm_sweep_if_settled();
        assert_eq!(box_labels(&state), ["fix/crash"]);
    }

    #[test]
    fn only_a_swept_checkout_with_a_branch_deletes_one() {
        // A checkout with nothing out: the sweep offers it nothing, the user marks it by
        // hand, and its label is a directory name that must never reach `git branch -d`.
        let mut state = sweeping();
        state.tree.repos[0].worktrees.push(WorktreeNode {
            branch: None,
            checkout_path: "/wt/app/scratch".into(),
            is_primary: false,
            open_workspace_id: None,
            track: None,
            panes: vec![],
        });
        state.replace_tree(state.tree.clone());
        state.set_working_trees(answers(&[
            ("/src/app", WorkingTree::Clean),
            ("/wt/app/feat-login", WorkingTree::Clean),
            ("/wt/app/fix-crash", WorkingTree::Clean),
            ("/wt/app/feat-wip", WorkingTree::Clean),
            ("/wt/app/scratch", WorkingTree::Clean),
        ]));
        select(&mut state, "scratch");
        state.handle_key(key(KeyCode::Char(' ')));
        ask(&mut state);

        let sweep = state.pending_sweep().expect("a question should be up");
        let deleting: Vec<(&str, bool)> = sweep
            .removals()
            .iter()
            .map(|r| (r.label(), r.delete_branch()))
            .collect();
        assert_eq!(deleting, [("fix/crash", true), ("scratch", false)]);
        assert_eq!(sweep.branches(), 1);
    }

    #[test]
    fn nothing_left_after_the_re_read_asks_nothing() {
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Enter));
        state.set_working_trees(answers(&[
            ("/src/app", WorkingTree::Clean),
            ("/wt/app/feat-login", WorkingTree::Clean),
            ("/wt/app/fix-crash", WorkingTree::Dirty),
            ("/wt/app/feat-wip", WorkingTree::Clean),
        ]));
        state.set_waiting(false);
        state.confirm_sweep_if_settled();

        assert!(state.pending_sweep().is_none());
        assert_eq!(
            state.message(),
            Some("no longer marked: fix/crash — nothing left to remove")
        );
        assert!(state.is_sweeping(), "the sweep is still there to mark in");
    }

    #[test]
    fn a_question_a_key_took_back_does_not_come_back_on_the_next_frame() {
        // The loop asks `confirm_sweep_if_settled` on every frame, and a no is a no.
        for code in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Down] {
            let mut state = sweeping();
            ask(&mut state);
            assert_eq!(state.handle_key(key(code)), Action::Consumed, "{code:?}");
            state.set_waiting(false);
            state.confirm_sweep_if_settled();
            assert!(
                state.pending_sweep().is_none(),
                "{code:?}: the question came back"
            );
        }
    }

    #[test]
    fn a_row_dropped_by_the_re_read_is_named_by_the_repository_its_key_names() {
        // Two repositories list one path, and the re-read finds a pane in the second's
        // checkout.
        let mut state = sweeping();
        let mut tree = state.tree.clone();
        tree.repos.push(RepoNode {
            repo_key: "/src/old/.git".into(),
            repo_root: "/src/old".into(),
            display_name: "me/old".into(),
            refs: Refs::Read,
            worktrees: vec![WorktreeNode {
                branch: Some("chore/deps".into()),
                checkout_path: "/wt/app/fix-crash".into(),
                is_primary: false,
                open_workspace_id: None,
                track: Some(Track::Gone),
                panes: vec![],
            }],
        });
        state.replace_tree(tree);
        assert_eq!(state.chosen().len(), 2, "both rows at the path are offered");

        state.handle_key(key(KeyCode::Enter));
        let mut re_read = state.tree.clone();
        re_read.repos[1].worktrees[0]
            .panes
            .push(pane("w5:p1", "codex", AgentStatus::Working));
        state.replace_tree(re_read);
        state.set_waiting(false);
        state.confirm_sweep_if_settled();

        assert_eq!(state.message(), Some("no longer marked: chore/deps"));
        assert_eq!(box_labels(&state), ["fix/crash"]);
    }

    #[test]
    fn a_checkout_that_left_the_tree_during_the_re_read_is_named_by_its_path() {
        // Another session removed it between `Shift-S` and `Enter`, so there is no row left
        // to take a label from.
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Enter));
        let mut without = state.tree.clone();
        without.repos[0].worktrees.remove(2);
        state.replace_tree(without);
        state.set_waiting(false);
        state.confirm_sweep_if_settled();

        assert!(state.pending_sweep().is_none());
        assert_eq!(
            state.message(),
            Some("no longer marked: /wt/app/fix-crash — nothing left to remove")
        );
    }

    #[test]
    fn a_message_already_on_the_prompt_line_survives_a_re_read_that_dropped_nothing() {
        // A removal started earlier can report a refusal while the re-read is out.
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Enter));
        state.set_message("could not remove feat/wip: fatal: refused".into());
        state.set_waiting(false);
        state.confirm_sweep_if_settled();

        assert_eq!(box_labels(&state), ["fix/crash"]);
        assert_eq!(
            state.message(),
            Some("could not remove feat/wip: fatal: refused")
        );
    }

    #[test]
    fn every_row_the_re_read_dropped_is_named_in_path_order() {
        // Two marks overruled by one re-read — a pane opened in one checkout, work written
        // into the other — and both named, so neither loss is silent.
        let mut state = sweeping();
        select(&mut state, "feat/login");
        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(state.chosen().len(), 2);

        state.handle_key(key(KeyCode::Enter));
        let mut busy = state.tree.clone();
        busy.repos[0].worktrees[1]
            .panes
            .push(pane("w2:p1", "claude", AgentStatus::Working));
        state.replace_tree(busy);
        state.set_working_trees(answers(&[("/wt/app/fix-crash", WorkingTree::Dirty)]));
        state.set_waiting(false);
        state.confirm_sweep_if_settled();

        assert!(state.pending_sweep().is_none());
        assert_eq!(
            state.message(),
            Some("no longer marked: feat/login, fix/crash — nothing left to remove")
        );
    }
}
