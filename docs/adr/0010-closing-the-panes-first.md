# 10. Close the panes, then remove the checkout

Status: accepted

## Context

[ADR 0008](./0008-removing-a-worktree.md) gave the panes view `Shift-D`, and refused it on a
checkout with panes in it: "there is nothing here that is safe to delete". That was true of
what the picker could do at the time. It could remove a checkout, and a checkout somebody is
working in is not something to remove out from under them.

But a finished worktree has panes in it. That is its ordinary end state rather than an
unusual one: the branch was opened as a pane, an agent ran in it, the work landed. Tidying it
away means closing the workspace by hand, opening the picker again, finding the row again,
and pressing `Shift-D` — a trip out to the shell to finish the thing the picker is for.

Unlike in ADR 0008, herdr can be asked this time. `worktree.remove` takes a `workspace_id`
and removes the worktree behind an open workspace, and a checkout with panes in it has one.
The symmetry with [ADR 0001](./0001-delegate-worktree-creation.md) — creating a worktree is
herdr's job, so removing one should be too — is available here where it was not there.

## Decision

`Shift-D` on a checkout with panes closes every pane in it and then runs the same
`git worktree remove` the empty case runs. herdr is not asked.

**`--workspace` is the wrong unit, and this plugin is the reason it is.** ADR 0001 has the
branches view create a worktree and then move its root pane *out* of the workspace herdr made
for it. A checkout's panes are therefore wherever the user sent them: beside the pane they
came from, in another tab, in another space. `worktree.remove --workspace` addresses one
workspace. The picker already knows which panes belong to a checkout — that grouping is the
whole of the panes view — so it closes those, wherever they ended up.

**One visible action must not carry two sets of rules.** herdr's removal has its own
`--force`; git's refusal to throw away uncommitted work is what ADR 0008 called the feature.
Sending the busy case through herdr and the empty case through git would make `Shift-D` mean
two different things depending on whether anything happened to be running — a difference a
person has to memorise rather than read.

**A dirty checkout is refused before the question**, alongside the two refusals ADR 0008
already makes. The picker now knows which checkouts hold uncommitted work, so it can say what
git would have said instead of closing the panes and finding out afterwards. This matters
here in a way it does not for an empty checkout: there, a refusal costs nothing; here, the
panes are already gone by the time git speaks.

**The confirmation stays one key and grows a list.** The box ADR 0008 designed names the
branch and the path. It now also names everything that stops:

```
┌──────────────────────────────────────┐
│ Delete this checkout?                │
│                                      │
│   fix/crash                          │
│   ~/.herdr/worktrees/app/fix-crash   │
│                                      │
│   these panes close:                 │
│   ● claude  working   w1:p2          │
│   · shell             w1:p3          │
│                                      │
│   y delete     any other key cancels │
└──────────────────────────────────────┘
```

Uncommitted work is still git's to protect and still protected. The one thing with no safety
net is whatever a working agent has in flight, and naming it is that net. ADR 0008 turned
down heavier ceremony because it pushes the user back to the shell, which is where they were
before any of this existed. That reason has not changed, and a list you read is not ceremony.

## Consequences

**`HerdrPort` gains `pane_close`.** It is the first call this plugin makes that takes
something out of the session rather than adding to it or rearranging it.

**The order is panes, then git, and it is not atomic.** The dirty check turns the common
failure into a refusal instead of a report, but it is a check and not a lock: a file written
between the check and the removal leaves the panes closed and the checkout still standing.
The picker says exactly that, in git's words, on that row — rather than implying the whole
thing was declined.

**Nothing closes a workspace.** herdr collapses a tab and a workspace that end up empty,
which is what makes ADR 0001's create-then-move leave no residue. That this also holds for
`pane.close` is an assumption CI cannot check here, so it belongs in the manual checklist in
`docs/en/troubleshooting.md`: remove a checkout whose panes were the last ones in their
workspace, and confirm no empty workspace is left behind.

**`SECURITY.md` changes again.** It says the plugin deletes a checkout you named and nothing
else. It now also closes panes you named, which stops whatever was running in them.

**Still no branch deletion, and still no `--force`.** The first is the sweep's business
([ADR 0011](./0011-what-may-be-swept.md)); the second is nobody's.
