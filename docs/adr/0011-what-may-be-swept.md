# 11. Sweep on what git knows, and let `gh` widen it

Status: accepted — amends [ADR 0003](./0003-git-first-gh-optional.md)

## Context

Checkouts accumulate. [ADR 0008](./0008-removing-a-worktree.md) says so — it is why the
picker stays open after a removal — and then offers one row at a time. Twenty finished
worktrees are twenty confirmations.

What stopped a sweep being obvious is that it has to answer a question the picker could not:
which of these is finished with. `Shift-D` never had to know. It acts on the row under the
cursor because a person put the cursor there.

The picker can answer it now. A branch carries `%(upstream:track)`, so one whose remote is
gone says so; a checkout carries whether its working tree is clean; and the tree says whether
anything is running in it. `gone` after `git fetch --prune` covers the ordinary GitHub flow
including a squash merge, because GitHub deletes the head branch on merge — when that setting
is on. When it is off, or when whoever merged it kept the branch, git has nothing to say and
`gh` does: the pull request is merged, or closed.

## Decision

A sweep mode in the panes view. It marks candidates, `Space` adds and removes a mark, and
`Enter` removes what is marked. Nothing is deleted that was not on the screen with a mark
against it.

**Marked by default:** the upstream is gone, the working tree is clean, and nothing is
running in the checkout. Where `gh` answers, also: the branch's pull request is merged or
closed, clean, and nothing running.

**`gh` may only widen the set.** It never clears a mark git put there, never gates the mode,
and never decides anything by itself. Without it the sweep runs on git alone, and the rows
where a pull request would have been consulted say so rather than looking like rows with
nothing to find.

This is the sentence in ADR 0003 that changes. "`gh` adds pull request numbers and titles and
nothing else" is no longer true — it also raises a candidate for deletion. What ADR 0003 was
actually protecting is untouched, and worth restating: `gh` missing, unauthenticated,
offline, or pointed at a repository GitHub has never heard of degrades to an empty list, and
nothing the picker can do goes away with it. `GhPort::pull_requests` still returns a `Vec`
and not a `Result`.

What is new, and is the price of this decision, is that a degraded `gh` is now *visible*: the
same repository sweeps fewer rows without it. That is why those rows say `PR unknown` rather
than nothing at all. A tool that quietly finds less when a dependency is missing is worse
than one that says which half it is missing.

**`git branch --merged` is not used.** It answers whether a tip is an ancestor of `HEAD`,
which both a squash merge and a rebase defeat, and squash is how most of these branches land.
It would mark almost nothing while looking like it had considered the question.

**Removal is `git worktree remove`, then `git branch -d`.** The first is the command
`Shift-D` runs, with no `--force`, so a checkout that turns out to be dirty is refused by git
and reported on its own row while the rest of the sweep carries on. ADR 0008 kept the branch
on purpose, and a sweep is the case where keeping it is the wrong half of the job: the local
branch would otherwise outlive its checkout in the branches view for ever.

**Never `-D`.** A squash-merged branch is not merged as far as `-d` is concerned, so `-d`
refuses exactly the branches the sweep is most confident about, and says so on the row: the
checkout went, the branch stayed. Somebody will want `-D` there. The reason to refuse is the
asymmetry — a checkout deleted by mistake is rebuilt by making it again, and a branch deleted
by mistake is recoverable only if it was pushed, which for a branch whose upstream is `gone`
is precisely what it no longer is. The sweep's confidence comes from a prune that may be
stale and a pull request that may have been reopened. That is enough to offer a mark. It is
not enough to pass a `-D`.

## Consequences

**`GhPort` needs pull request state, and a wider query.** Decoration asks for open pull
requests; the sweep asks about branches whose pull request is closed. `gh pr list --state
closed` is the heavier call — `closed` rather than `all` because `gh` counts the open ones
against the same window and the sweep then discards them, so asking for everything spends the
window on answers it does not want. It is made when the sweep is entered rather than when the
picker
opens. The branches view keeps the cheap query it has, and anyone who never sweeps never pays
for the other one.

**A default mark is a suggestion with its reason attached.** Each marked row says why —
`gone`, `PR #123 merged` — because a mark whose reason is invisible is one the user either
trusts blindly or clears wholesale. There is no "select all" reaching rows the sweep did not
mark: widening the selection is `Space`, one row at a time, which is the same act as
disagreeing with a mark.

**Two ways to delete a checkout, one path.** The sweep runs ADR 0008's removal once per
marked row, refusals included. A checkout with panes in it is never swept: closing somebody's
panes is [ADR 0010](./0010-closing-the-panes-first.md)'s single deliberate act, and a batch
is not where it belongs.

**`SECURITY.md` gains branch deletion**, which it currently promises never happens.
