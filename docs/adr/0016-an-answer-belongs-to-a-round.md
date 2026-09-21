# 16. A background answer belongs to a round, not to a thread

Status: accepted — carries out [ADR 0007](./0007-stay-up-while-working.md)

## Context

Two questions in the panes view are too slow to ask on the way to a frame. Whether a checkout
is holding uncommitted work costs a walk of the working tree, once per checkout — `Dirty`.
What became of a repository's pull requests costs a `gh` call, once per repository —
`Settled`. Both are asked off the loop and filled in as answers land, which is what
[ADR 0007](./0007-stay-up-while-working.md) asks for.

Both can also be told to forget. `r` reloads, and means "about things as they are now": a
working tree the user has been editing since, a pull request that landed while the picker was
up. Entering a sweep asks again too, of the repositories `gh` could not answer for.

A thread already running cannot be called back. So at the moment of forgetting there are
answers in flight that are about the world as it was, and they are indistinguishable on
arrival from answers about the world as it is. Worse, they need not arrive first: `mpsc`
hands over in whatever order the calls finish, so a slow call from before the reload can land
after a fast one from after it. Dropping whatever arrives before the next ask goes out is not
a rule that holds.

## Decision

Every reply carries the round of asking it belongs to, and a round is dropped when it is no
longer current.

Each asker keeps a `generation`, bumped whenever answers are thrown away. Every spawned call
captures the generation current when it started and sends it back with its reply, and `drain`
keeps a reply only while its generation is the one in hand. Threads already running are left
alone.

**Dropped on the round, not on whether the subject is still listed.** Whether a checkout or a
repository is still in the tree is a different question with a different answer, and it is
asked separately where it matters — a spinner is shown for what is on screen, not for what is
in flight.

**Forgetting and asking are one call where they cannot be usefully separated.** `Dirty::reask`
bumps, clears and asks: a `Dirty` that had forgotten and not yet asked would sit with its
spinner turning over a list it will never say anything about. `Settled::forget` only bumps
and clears, because the next `ask` is what a sweep entered later will do; `forget_failures`
is the narrower version that throws away the refusals and leaves the answers.

## Consequences

**A round that was asked and abandoned still costs its processes.** They run to completion and
their answers are discarded. Capping how many are in flight — `MAX_IN_FLIGHT` in `Dirty` —
is what keeps a burst of reloads from filling a laptop with git processes; `Settled` needs no
cap because there are as many repositories as the user has panes open in.

**A stale answer cannot win.** That is the property the generation buys, and the one worth
checking when either module is changed: the failure it prevents is silent, correct-looking,
and arrives in whatever order the network felt like.

**The same shape twice, deliberately.** `Dirty` and `Settled` ask different questions of
different tools at different rates, and share only this. Merging them would put a queue and a
slot count on the one that needs neither.
