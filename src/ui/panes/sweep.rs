//! The sweep: judging every checkout at once, and what the user marks in it.
//!
//! Entering is a mode rather than a filter. The list widens to every checkout the sweep has
//! something to say about, each one is judged, and what was judged removable starts marked.
//! Leaving forgets all of it, which is why the marks live in the sweep and not in the
//! picker — `docs/adr/0011-what-may-be-swept.md`.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::model::{CheckoutPath, RepoKey};
use crate::domain::rows::RowRef;
use crate::domain::sweep::{self, Mark, RepoRoot};
use crate::port::SettledPullRequests;
use crate::ui::panes::*;

impl PanesState {
    /// What a sweep would do with every checkout, or `None` when no sweep is on.
    ///
    /// Run on every rebuild rather than kept, because everything it decides on moves while
    /// the sweep is on screen: a working tree answers, a removal starts, `gh` lands, `r`
    /// reads the tree again. The user's changes are what is kept; the judgement is not.
    pub(super) fn judge(&self) -> Option<BTreeMap<(RepoKey, CheckoutPath), Mark>> {
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
    pub(super) fn set_sweeping(&mut self, sweeping: bool) -> Action {
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
            // A row usually goes because a pane opened in it or a file was written, and the
            // rows themselves show that. It can also go because the repository it was in is
            // not there to be read any more, and then the bare paths are the whole of what
            // is left — `nothing left to remove` reads as a sweep that found nothing rather
            // than one that lost its ground. Only a listing that failed can take rows away,
            // so no other condition is named here.
            let unlisted = self.conditions().into_iter().find(|condition| {
                matches!(condition, crate::domain::notice::Condition::Unlisted { .. })
            });
            self.message = Some(match (unlisted, after.is_empty()) {
                (Some(condition), _) => format!("{named} — {}", words::condition(&condition)),
                (None, true) => format!("{named} — nothing left to remove"),
                (None, false) => named,
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
    pub(super) fn label_of(&self, key: &(RepoKey, CheckoutPath)) -> String {
        match self.tree.find_checkout(key) {
            Some((_, worktree)) => worktree.label().to_string(),
            None => key.1.as_str().to_string(),
        }
    }

    /// Add or remove the mark on the row under the cursor.
    pub(super) fn flip_mark(&mut self) -> Action {
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
        if let Some(refusal) = words::sweep_refusal(mark) {
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
}

#[cfg(test)]
mod tests;
