//! What order the branch list is in.
//!
//! Three keys rather than one fixed order, because the question "which branch do I want"
//! has more than one shape: what is already running, what I touched last, or where a name
//! is in the alphabet. Each key has a direction it is worth reading in before anyone
//! reverses it, and switching key returns to that direction — nobody asking for "by date"
//! meant "oldest first".

use std::cmp::Ordering;

use crate::domain::resolve::{BranchEntry, BranchState};

/// What the list is ordered by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortKey {
    /// What the branch currently is: running, checked out, local, remote-only. The default,
    /// because it puts the work you already have in front of the work you might start.
    #[default]
    State,
    /// Committer date of the branch's tip.
    Updated,
    /// The branch name.
    Name,
}

impl SortKey {
    /// The order `Ctrl-O` walks through.
    const CYCLE: [SortKey; 3] = [SortKey::State, SortKey::Updated, SortKey::Name];

    fn next(self) -> Self {
        let at = Self::CYCLE.iter().position(|key| *key == self).unwrap_or(0);
        Self::CYCLE[(at + 1) % Self::CYCLE.len()]
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::State => "state",
            SortKey::Updated => "updated",
            SortKey::Name => "name",
        }
    }

    /// Whether the key reads high-to-low until someone reverses it. A date and a state are
    /// worth having busiest-and-newest first; a name is only ever worth having a to z.
    fn descends_naturally(self) -> bool {
        !matches!(self, SortKey::Name)
    }
}

/// A key and which way round it is being read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Order {
    pub key: SortKey,
    /// Set by the user, on top of the key's natural direction.
    pub reversed: bool,
}

impl Order {
    /// The next key, back at its natural direction.
    pub fn cycle(self) -> Self {
        Self {
            key: self.key.next(),
            reversed: false,
        }
    }

    pub fn reverse(self) -> Self {
        Self {
            reversed: !self.reversed,
            ..self
        }
    }

    /// `updated \u{2193}`. The arrow is about the values, not the rows: `\u{2193}` is
    /// descending, so it means newest, busiest, or z first.
    pub fn label(self) -> String {
        let descending = self.key.descends_naturally() != self.reversed;
        let arrow = if descending { '\u{2193}' } else { '\u{2191}' };
        format!("{} {arrow}", self.key.label())
    }

    pub fn compare(self, a: &BranchEntry, b: &BranchEntry) -> Ordering {
        // A branch nobody has fetched has no date at all. Those sink to the bottom whichever
        // way the list points: reversing an order should not fill the top of the screen with
        // the rows that have the least to say.
        if self.key == SortKey::Updated {
            match (a.committed_at, b.committed_at) {
                (None, Some(_)) => return Ordering::Greater,
                (Some(_), None) => return Ordering::Less,
                _ => {}
            }
        }

        let primary = match self.key {
            SortKey::State => rank(&a.state)
                .cmp(&rank(&b.state))
                .then_with(|| newest_first(a, b)),
            SortKey::Updated => newest_first(a, b),
            SortKey::Name => a.name.cmp(&b.name),
        };
        let primary = if self.reversed {
            primary.reverse()
        } else {
            primary
        };
        // The name breaks every tie, always the same way, so rows that are equal by the key
        // keep their places instead of shuffling between redraws.
        primary.then_with(|| a.name.cmp(&b.name))
    }

    pub fn sort(self, entries: &mut [BranchEntry]) {
        entries.sort_by(|a, b| self.compare(a, b));
    }
}

/// Where a state sits when the list is ordered by state: work in progress first, then
/// checkouts, then refs, then what is only on the remote.
fn rank(state: &BranchState) -> u8 {
    match state {
        BranchState::New => 0,
        BranchState::LivePane { .. } => 1,
        BranchState::IdleWorktree { .. } => 2,
        BranchState::LocalRef => 3,
        BranchState::RemoteOnly => 4,
    }
}

fn newest_first(a: &BranchEntry, b: &BranchEntry) -> Ordering {
    b.committed_at.cmp(&a.committed_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, state: BranchState, committed_at: Option<i64>) -> BranchEntry {
        BranchEntry {
            name: name.into(),
            state,
            subject: None,
            committed_at,
            pull_request: None,
            upstream_gone: false,
        }
    }

    /// One of each state, deliberately shuffled.
    fn entries() -> Vec<BranchEntry> {
        vec![
            entry("zeta", BranchState::LocalRef, Some(300)),
            entry("never-fetched", BranchState::RemoteOnly, None),
            entry(
                "alpha",
                BranchState::LivePane {
                    pane_id: "w1:p1".into(),
                    checkout_path: "/wt/alpha".into(),
                },
                Some(100),
            ),
            entry(
                "middle",
                BranchState::IdleWorktree {
                    checkout_path: "/wt/middle".into(),
                },
                Some(200),
            ),
        ]
    }

    fn names(order: Order) -> Vec<String> {
        let mut list = entries();
        order.sort(&mut list);
        list.into_iter().map(|entry| entry.name).collect()
    }

    #[test]
    fn the_default_is_the_order_the_picker_has_always_had() {
        let order = Order::default();
        assert_eq!(order.key, SortKey::State);
        assert!(!order.reversed);
        assert_eq!(names(order), ["alpha", "middle", "zeta", "never-fetched"]);
    }

    #[test]
    fn updated_is_newest_first_and_reversing_it_is_oldest_first() {
        let updated = Order::default().cycle();
        assert_eq!(updated.key, SortKey::Updated);
        assert_eq!(names(updated), ["zeta", "middle", "alpha", "never-fetched"]);
        assert_eq!(
            names(updated.reverse()),
            ["alpha", "middle", "zeta", "never-fetched"]
        );
    }

    #[test]
    fn a_branch_with_no_date_stays_at_the_bottom_in_both_directions() {
        // Reversing "by date" must not promote the rows that have no date to the top; they
        // are the ones with the least to say about themselves.
        let updated = Order {
            key: SortKey::Updated,
            reversed: false,
        };
        assert_eq!(names(updated).last().unwrap(), "never-fetched");
        assert_eq!(names(updated.reverse()).last().unwrap(), "never-fetched");
    }

    #[test]
    fn name_runs_a_to_z_before_it_is_reversed() {
        let name = Order::default().cycle().cycle();
        assert_eq!(name.key, SortKey::Name);
        assert_eq!(names(name), ["alpha", "middle", "never-fetched", "zeta"]);
        assert_eq!(
            names(name.reverse()),
            ["zeta", "never-fetched", "middle", "alpha"]
        );
    }

    #[test]
    fn changing_key_puts_the_direction_back_to_that_keys_natural_one() {
        let reversed_state = Order::default().reverse();
        assert!(reversed_state.reversed);
        assert!(
            !reversed_state.cycle().reversed,
            "asking for another key should not inherit the last key's reversal"
        );
    }

    #[test]
    fn the_cycle_returns_to_where_it_started() {
        let start = Order::default();
        assert_eq!(start.cycle().cycle().cycle(), start);
    }

    #[test]
    fn the_arrow_describes_the_values_rather_than_the_rows() {
        assert_eq!(Order::default().label(), "state \u{2193}");
        assert_eq!(Order::default().cycle().label(), "updated \u{2193}");
        assert_eq!(
            Order::default().cycle().reverse().label(),
            "updated \u{2191}"
        );
        // a to z is ascending, so the name key points the other way to begin with.
        assert_eq!(Order::default().cycle().cycle().label(), "name \u{2191}");
        assert_eq!(
            Order::default().cycle().cycle().reverse().label(),
            "name \u{2193}"
        );
    }

    #[test]
    fn ties_are_broken_by_name_so_rows_never_shuffle_between_redraws() {
        let mut list = vec![
            entry("b", BranchState::LocalRef, Some(100)),
            entry("a", BranchState::LocalRef, Some(100)),
        ];
        Order::default().sort(&mut list);
        assert_eq!(list[0].name, "a");

        // Even reversed: the tiebreak is not part of what gets flipped.
        let mut list = vec![
            entry("b", BranchState::LocalRef, Some(100)),
            entry("a", BranchState::LocalRef, Some(100)),
        ];
        Order::default().reverse().sort(&mut list);
        assert_eq!(list[0].name, "a");
    }
}
