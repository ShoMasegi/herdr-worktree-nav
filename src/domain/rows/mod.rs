//! The rows the panes picker draws: what the tree flattens to, and where the cursor may
//! stop.
//!
//! [`Row`] and [`ViewOptions`] are here with the list they describe. [`flatten()`] is the
//! shape — herdr's own session navigator's, where a row that did not match the filter
//! itself is kept as context rather than removed — and [`nav`] is what the arrow keys do to
//! it.
//!
//! Pure, and it carries no wording: a row says what it is and what is true of it, and what
//! any of that reads as is [`ui::words`](crate::ui::words)'s.

use std::collections::BTreeMap;

use crate::domain::model::{CheckoutPath, RepoKey, WorkingTree};
use crate::domain::sweep::Mark;
use crate::port::{AgentStatus, Track};

mod flatten;
mod nav;

#[cfg(test)]
pub(crate) mod fixtures;

pub use flatten::flatten;
pub use nav::{groups, next_row, previous_row, selectable, step_group};

/// Which node of the tree a row stands for. Indices point into
/// [`domain::model::Tree`](crate::domain::model::Tree).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRef {
    Repo(usize),
    Worktree(usize, usize),
    Pane(usize, usize, usize),
    /// The group holding panes that are not inside any git work tree.
    UngroupedRepo,
    Ungrouped(usize),
}

impl RowRef {
    /// Whether this row heads a group — the navigator's "workspace" role, which is what
    /// gets the blank line above it.
    pub fn is_group(self) -> bool {
        matches!(self, RowRef::Repo(_) | RowRef::UngroupedRepo)
    }

    /// The navigator's "tab" role: a middle level that is neither a group nor a leaf.
    pub fn is_worktree(self) -> bool {
        matches!(self, RowRef::Worktree(..))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub reference: RowRef,
    /// Tree depth: 0 for a group, 1 for a worktree, 2 for a pane under one.
    pub depth: u8,
    /// What the row is called, as herdr and git have it: a repository's name, a checkout's
    /// branch, a pane's agent. `None` only for a pane herdr tracks no agent in, and what
    /// that reads as is [`ui::words`](crate::ui::words)'s to decide, as is whether a name
    /// is all a row is called — see [`ui::words::row_label`](crate::ui::words::row_label).
    pub name: Option<String>,
    /// How many panes the group holds, and none on a row that is not a group. Counted here
    /// because the count is over the whole subtree, which the row alone cannot see.
    pub panes: usize,
    /// The right-hand column: where the thing is. A checkout path for a worktree, a pane id
    /// for a pane, and for a repository its root — but only while it is folded, since the
    /// main checkout directly beneath otherwise repeats it.
    pub meta: String,
    pub status: AgentStatus,
    /// A worktree with no pane in it. Called out beside the label, since its meta column is
    /// taken by the checkout path.
    pub is_idle: bool,
    /// A checkout whose removal has been started and is still running somewhere else. It
    /// takes the place of the `no pane` note, since it is the more urgent fact about the
    /// same row — see `docs/adr/0014-removing-outlives-the-picker.md`.
    pub is_removing: bool,
    /// What git said about this checkout's working tree, and `None` while nobody has been
    /// told. The answer costs a process per checkout and arrives after the first frame, so
    /// `None` is the ordinary state of the first frame — and a marker that is wrong for a
    /// moment is worse than one that is late.
    ///
    /// Not a fact about the checkout: in a live `PanesState` this lags, because
    /// `set_working_trees` does not rebuild the list for an answer no row would draw, so a
    /// checkout that has just answered `Clean` keeps the `None` it was flattened with until
    /// something else rebuilds. The two render identically, which is what makes that sound.
    /// Only `marks` reads this, and telling the two apart is [`ViewOptions::working_trees`]'
    /// job.
    pub working_tree: Option<WorkingTree>,
    /// What git said about this checkout's branch against its upstream.
    pub track: Option<Track>,
    /// The row the session is currently on, marked with a caret in the gutter.
    pub is_current: bool,
    /// Whether this row matched the active filter, as opposed to being kept as ancestor
    /// context or as part of a matching group's subtree. Always true with no filter.
    pub matched: bool,
    /// What a sweep would do with this checkout, or `None` when no sweep is on — which is
    /// most of the time, and is why this is an `Option` rather than a fourth state of
    /// `Mark`. Only worktree rows ever carry one: a sweep deletes checkouts, and a group or
    /// a pane is neither.
    pub sweep: Option<Mark>,
}

impl Row {
    /// Whether the cursor stops here.
    ///
    /// Only rows that stand for somewhere to go: a pane, and a checkout with nothing
    /// running in it. A repository is a heading, and a checkout that already has panes is
    /// answered by the panes listed directly under it — stopping on either would only make
    /// the arrow keys longer to press.
    ///
    /// A sweep changes what the cursor is for, so it changes this: every checkout is a row
    /// with an answer on it, the ones the sweep refuses included, since a refusal is asked
    /// for by pressing `Space` on the row. One refusal stays out of reach: the cursor does
    /// not stop on a checkout being removed, sweep or no sweep, and `deleting` on the row is
    /// what says so.
    pub fn is_selectable(&self) -> bool {
        if self.is_removing {
            // A second `Shift-D` would race the first, and Enter would open a checkout being
            // deleted underneath.
            return false;
        }
        match self.reference {
            RowRef::Pane(..) | RowRef::Ungrouped(_) => true,
            RowRef::Worktree(..) => self.is_idle || self.sweep.is_some(),
            RowRef::Repo(_) | RowRef::UngroupedRepo => false,
        }
    }
}

/// One rendered line. Blank lines separate groups and cannot be selected — the same shape
/// herdr's navigator uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayLine {
    Spacer,
    Row(usize),
}

/// Interleave a blank line before every group but the first.
pub fn display_lines(rows: &[Row]) -> Vec<DisplayLine> {
    let mut lines = Vec::with_capacity(rows.len() * 2);
    for (index, row) in rows.iter().enumerate() {
        if row.reference.is_group() && !lines.is_empty() {
            lines.push(DisplayLine::Spacer);
        }
        lines.push(DisplayLine::Row(index));
    }
    lines
}

/// Narrow the list to one agent state, the way the navigator's `b`/`w`/`i`/`d` keys do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    Blocked,
    Working,
    Idle,
    Done,
}

impl StateFilter {
    pub fn status(self) -> AgentStatus {
        match self {
            StateFilter::Blocked => AgentStatus::Blocked,
            StateFilter::Working => AgentStatus::Working,
            StateFilter::Idle => AgentStatus::Idle,
            StateFilter::Done => AgentStatus::Done,
        }
    }

    fn matches(self, status: AgentStatus) -> bool {
        self.status() == status
    }
}

#[derive(Debug, Clone, Default)]
pub struct ViewOptions {
    pub query: String,
    pub state_filter: Option<StateFilter>,
    /// Whether the ordinary panes view omits worktrees that contain no pane.
    /// A sweep still includes them because they are its main candidates.
    pub hide_worktrees_without_panes: bool,
    /// Checkout paths whose removal has been started and has not reported back. Passed in
    /// rather than read, because this module does not touch the outside world.
    pub removing: Vec<CheckoutPath>,
    /// What git has said about each working tree so far. A checkout that has not answered
    /// is absent, which is a different fact from `Clean` and decides different things — see
    /// [`domain::model::WorkingTree`](crate::domain::model::WorkingTree).
    pub working_trees: BTreeMap<CheckoutPath, WorkingTree>,
    /// What a sweep would do with each checkout, by repository and checkout path, or `None`
    /// when no sweep is on. Worked out once by
    /// [`domain::sweep::marks`](crate::domain::sweep::marks) and handed here: the same answer
    /// decides what is drawn and what is deleted.
    pub sweep: Option<BTreeMap<(RepoKey, CheckoutPath), Mark>>,
}

impl ViewOptions {
    fn filtering(&self) -> bool {
        !self.query.trim().is_empty() || self.state_filter.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::rows::fixtures::*;

    #[test]
    fn a_blank_line_separates_each_group_but_not_the_first() {
        let rows = flatten(&tree(), &ViewOptions::default());
        let lines = display_lines(&rows);
        assert_eq!(lines[0], DisplayLine::Row(0), "no leading blank line");
        let spacers: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| **line == DisplayLine::Spacer)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(spacers.len(), 2, "one blank line per group boundary");
        assert_eq!(
            lines[spacers[0] + 1],
            DisplayLine::Row(7),
            "me/site follows"
        );
        assert_eq!(
            lines[spacers[1] + 1],
            DisplayLine::Row(10),
            "then the panes in no repository"
        );
    }
}
