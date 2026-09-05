# 14. A removal outlives the picker, and herdr's notification reports it

Status: accepted — extends [ADR 0008](./0008-removing-a-worktree.md)

## Context

`Shift-D` runs `git worktree remove` inside the panes loop, between one `event::read` and the
next. A checkout is a whole working tree: git walks it looking for uncommitted work and
untracked files, refuses if it finds any, and otherwise deletes the lot. On a repository of
any size that is seconds, and for those seconds the loop is not drawing and not reading keys.
The picker is frozen.

That is the failure [ADR 0007](./0007-stay-up-while-working.md) fixed at the other end of the
plugin, where opening a branch went blank for a fetch and a checkout. Its answer was a worker
thread with the picker still on screen narrating the step. It would fix the freeze here too.

It would not fix the rest of it. Opening a branch ends in something the user is waiting for —
a pane, in the place they asked for it — so staying to watch is the point. A removal ends in
an absence. The natural move after `y` is to close the picker, and a design that keeps the
process alive until the deletion finishes makes that the one move you cannot make.

## Decision

`y` starts the removal in a process of its own and goes straight back to the loop.

**A session of its own, not merely a thread.** The child is
`herdr-worktree-nav remove <repo_root> <checkout_path> <branch>`, spawned through `setsid`.
This is not caution for its own sake: herdr kills a closed pane's process group. Measured
against herdr 0.7.4 by splitting a pane, leaving two children in it and closing it — the
child in the pane's process group was killed, the child in a session of its own survived. A
removal killed halfway through leaves a checkout that is neither there nor gone, which is the
one outcome this plugin has no way to explain.

**The child reports, always, and it reports through `notification.show`.** Not the picker,
even when the picker is up: the picker may be on the branches view, or already closed, and
neither the child nor the loop can tell whether a line written down the pipe was ever read by
anyone. One reporter that always speaks beats two that each assume the other did.

- removed: `removed <branch>`, the path in the body, and no sound. Tidying up is done often;
  a chime for every checkout that goes is noise.
- refused: `could not remove <branch>`, git's own words in the body — with what it cost to
  reach them, once closing panes became part of getting there — and the sound. This is the
  one that has to reach someone who has stopped looking.

**The picker shows it while it is there, and adds only what the toast cannot.** The row's
`no pane` becomes `deleting ⠻` and stops being selectable, so the cursor steps over it and a
second `Shift-D` cannot reach it. A refusal goes on the prompt line, naming its own checkout
because several can be in flight. A success is not announced at all: the row leaving the list
is the report.

**A notification herdr declines is accepted in silence.** `notification.show` answers
`shown: false` with a reason — `disabled`, `no_foreground_client`, `rate_limited`, `busy`.
Routing around a user who has turned notifications off is overruling them. And the report is
not actually lost: a refused removal leaves the checkout standing, so the row is there next
time, and `Shift-D` on it gives git's reason inline.

That last sentence stopped being wholly true when
[ADR 0010](./0010-closing-the-panes-first.md) was implemented. A refusal that came after the
panes were closed says so — `— its 2 panes were closed first` — and that clause is the half
the backstop cannot reproduce: next time, the row has no panes to close, so asking again
gives git's reason without it. The checkout still being there is still a report; what it has
stopped being is the whole of one.

**Nothing is queued.** One process per removal, started when the key is pressed. A queue
would have to be owned by the picker, and a queue owned by something the user is free to
close is a queue that can strand what it has not started yet.

**The confirmation box does not change.** It says what is about to go; what happens after `y`
is said by the row, in the place the user is already looking.

## Consequences

**`HerdrPort` gains `notify`.** Every other call on that port does something to the session.
This one says something to the person using it, and it is the first thing this plugin can do
after its own window has gone.

**A fourth mode on the binary.** `action`, `pane`, `dump`, and now `remove` — the first that
this plugin invokes rather than herdr. It is on the same binary so that the removal and the
words that report it come from the same `GitPort` and the same `HerdrPort` the picker uses,
rather than being reassembled out of shell quoting.

**A failure no longer reaches `herdr plugin log list`.** ADR 0007 made a point of raising an
error again on the way out so that what the user read is also what the log got. herdr collects
the stderr of processes it started, and it did not start this one. The toast is the report,
and the checkout that is still there is the backstop. This is the price of the decision and
it is worth naming: a removal that fails while nobody is watching is thinner in the record
than it used to be.

**The picker no longer knows the list is stale.** It reloads when a child finishes and it is
still up to hear about it. When it is not, the next time it opens is when the list catches up
— which is what it has always done, since a checkout can go for reasons that have nothing to
do with this plugin.

**The panes loop grows a clock, and only while it needs one.** `event::read` blocks until a
key arrives, which is right for a view that has nothing running. With a removal in flight it
polls on the same 80 ms tick the branches view uses, so the spinner turns; with none, it goes
back to blocking and draws no frames at all.

**Still no `--force`, and still no branch deletion.** Neither is affected by where the
removal runs. The first is nobody's ([ADR 0008](./0008-removing-a-worktree.md)) and the
second is the sweep's ([ADR 0011](./0011-what-may-be-swept.md)).

**The manual checklist grows two lines.** That a toast arrives after the picker has been
closed on top of a running removal, and that the removal completes at all, are both things CI
cannot see.
