# 7. Keep the picker on screen while it opens a branch

Status: accepted

## Context

Choosing a branch used to end the picker. The loop broke, `ratatui::try_restore()` put the
terminal back, and only then did the fetch, the worktree creation and the pane move run.

herdr keeps the popup up until the process exits, and a restored terminal in a fresh pane is
empty. So the user pressed Enter and got an empty box, framed by herdr, for as long as the
work took — a fetch across the network plus a checkout of a whole working tree, which is
seconds. It looks exactly like a plugin that has hung, and it was reported as one.

A failure was worse than that. The error went to stderr, the process exited, herdr closed the
popup — so the popup vanished with no explanation, which is indistinguishable from having
worked. The only trace was `herdr plugin log list`.

## Decision

The work moves inside the loop, onto a worker thread, and the picker stays on screen and
narrates it. The destination list and its preview stay exactly where they were; only the
prompt line and the key hint change. The prompt becomes a spinner and the current step, and
the highlighted row below it is still the destination being acted on.

On failure the picker holds the screen with the error on the step that produced it, and waits
for a key.

## Consequences

**`HerdrPort` gains a `Sync` bound.** The worker needs `&dyn HerdrPort`, and `&T` is `Send`
only when `T: Sync`. The only implementation is `SocketHerdr`, whose only obstacle was a
`Cell<u64>` request counter; it is an `AtomicU64` now. `GitPort` and `GhPort` were already
`Sync` for the same reason.

**The spinner is driven by the draw loop, not a clock.** The loop already wakes every 80 ms
to poll for keys, so one frame per draw is all the animation needs — and `domain` stays free
of the clock it is not allowed to read.

**`Ctrl-C` works only up to and including the fetch.** `Stage::interruptible` draws that
line, and it is not about patience. A fetch writes only to `refs/remotes` and can be
abandoned. Once herdr has been asked for a worktree, quitting before the pane has been moved
would leave a workspace herdr made and nobody moved — the residue that
[ADR 0001](./0001-delegate-worktree-creation.md)'s create-then-move design exists to avoid.
The key hint says which of the two states the picker is in.

**Cancelling leaves the process rather than the loop.** `std::thread::scope` joins its
threads before the scope can return, so breaking out of the loop would wait for exactly the
fetch the user just asked to stop. The cancel path restores the terminal and exits. It is
safe precisely because it is only reachable while `interruptible` is true: there is nothing
half-made to tidy up, and the `git fetch` that outlives us only ever writes `refs/remotes`.

**A failure is shown and then raised again.** The picker displays it, and `run` returns it on
the way out so that what the user read is also what `herdr plugin log list` gets. Showing it
and swallowing it would have traded one kind of silence for another.

## The bug this uncovered

Putting errors in front of the user immediately proved the point: the first real failure
reported was `<the repository> is not a git repository, but was expected to be` — for a
repository that was perfectly fine, when what had actually happened was that the fetch could
not reach the remote.

The git adapter treated exit code 128 as "not a git repository". 128 is git's catch-all for
every fatal error, and an unreachable remote is one of them. It now requires git to have
actually said so. That error had been reachable all along; nobody had seen it, because it was
printed to a pane that herdr was about to close.
