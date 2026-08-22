# 8. Remove a worktree with git, and ask first

Status: accepted

## Context

The panes view lists checkouts that have nothing running in them, marked `no pane`. They
accumulate: every branch anyone has worked on leaves one behind, and until now the picker
could only add to the pile. Tidying up meant leaving the picker, finding the path, and
running `git worktree remove` by hand — with the path the picker had just been showing.

This is the first thing this plugin does that cannot be undone by doing it again the other
way round. [ADR 0001](./0001-delegate-worktree-creation.md) settled that creating a worktree
is herdr's job; the obvious symmetry would be that removing one is too.

It is not. `worktree.remove` takes a `workspace_id` and removes the worktree behind an open
workspace. A checkout with nothing running in it has no workspace, which is precisely the
kind this is for. herdr cannot be asked.

## Decision

`Shift-D` on a `no pane` row asks, and `y` runs `git worktree remove <path>` from the
adapter. Anything else is a no.

**No `--force`.** git refuses a checkout that has uncommitted changes or untracked files, and
that refusal is the feature: the picker has no business deciding that work nobody has
committed is disposable. What git says is what the user reads.

**The branch stays.** Removing a worktree and deleting a branch are different sizes of thing.
A checkout is rebuilt by making it again; a branch that was never pushed is not.

**Two refusals happen before the question**, because asking "are you sure?" about something
that cannot happen is worse than saying so:

- a pane row, or a checkout that has panes — there is nothing here that is safe to delete;
- the repository's own checkout — git cannot remove a main working tree, and it is not a
  worktree in the first place.

**The picker stays open.** Tidying up comes in batches, so the removal happens inside the
loop and the list reloads under the cursor rather than the picker closing over its own work.

## Consequences

`SECURITY.md` used to say this plugin never deletes anything. It now says what it does
delete, and what it will not: a checkout you asked it to remove by name, never a branch,
never anything with uncommitted work in it.

The confirmation is one key rather than a typed branch name. This is housekeeping, done
often, on things whose whole purpose was to be temporary; making the user type
`feat/hbr-51-grant-table-privileges` each time would push them back to the shell, which is
where they were before this existed. The question takes the whole prompt line and the key
hint says `y remove  any other key cancels`, so the one key it does take is a deliberate one.

`Shift-D` rather than `d`, which is the `done` state filter, or `x`, which is unshifted and
sits next to keys that move. The shift is the point: a destructive key should be slightly
harder to hit than the ones beside it.
