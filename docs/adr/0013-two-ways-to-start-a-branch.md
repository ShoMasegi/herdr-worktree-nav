# 13. Two ways to start a branch, and why they are not one

Status: accepted

## Context

The branches view could already create a branch: type a name that matches nothing and the
list offers `+ create`, which cuts a worktree from `HEAD`. That is the right base for
starting something off the mainline and the wrong one for the far more common ask — *this
branch, but a new one*. Doing that meant leaving the picker for a terminal.

The obvious move is to make the offer base on whatever the cursor is on, and have one way to
create a branch instead of two.

It does not work. The offer only exists while a query is being typed, because that is what
produces it — `refilter` sets it from the query, and in the branch list a non-empty query
always means the search box has the keyboard (`Esc` clears both; `Enter` chooses rather than
committing the search). So while the offer is on screen:

- The cursor is wherever the fuzzy match happened to leave it, which is row 0 of a list the
  user narrowed to find a *name*, not to pick a base.
- The offer sits last in whatever order is in force. Arrow down to it and the row under the
  cursor **is** the offer, so the branch would be its own base.

A base chosen by a cursor that moved for unrelated reasons is worse than a base that is
always `HEAD`, because it is not predictable.

## Decision

Two gestures, each naming its base.

- **Type a name that does not exist** → `+ create`, cut from `HEAD`. Unchanged.
- **`n` on a branch** → a prompt, `+ new branch from <base>:`, cut from that branch.

`n` is the panes view's key for "make a new thing out of the row the cursor is on" — there a
pane in the checkout under the cursor, here a branch off the branch under the cursor.

The prompt names the base in words rather than only leaving it highlighted in the list. What
separates the two paths is precisely which commit the branch starts at, so the screen that
asks for the name is the screen that has to say it.

Naming is its own step rather than a mode of the branch list. `Esc` from the destination
comes back to the name still typed, the way `Esc` from the destination has always come back
one step rather than out.

## Consequences

**The base is settled when `n` is pressed, and the list underneath goes inert.** It stays on
screen — the row being cut from is on it — but `j`/`k` and `Ctrl-N`/`Ctrl-P` do nothing while
a name is being typed. A cursor that could still move would let the base change under a
question that already named it.

**A never-fetched base is fetched first.** There is no local ref to cut from, so it is
`git fetch origin <base>` and then `origin/<base>` — the same answer choosing that row on its
own has always given. This uncovered a latent bug: `open` read what to fetch off the branch
being *created*, which was right only because the two names were the same for every path that
existed. It reads it off the base now.

**`Chosen` is two shapes, and is not a `BranchState`.** A `NewFrom` variant on `BranchState`
would have been fewer lines, and would have put a state that can never be a row into the enum
every renderer matches on to draw rows — a glyph, a state label, and a sort key would all
have needed an arm for something no list can contain.

**The offer to create cannot be used as a base**, and is guarded rather than merely
unreachable. Today the guard cannot fire, for the reason the whole design rests on: letters
are text while the offer is on screen. If a search ever survives its own keystrokes, the
guard is what stops a `git worktree add` based on a ref that does not exist.

**`Tab` moved down the key hint ladder.** Adding `n new branch` pushed the widest rung past
what a narrow pane fits, and the rung below it had dropped `⇥ panes` while keeping
`i order` and `shift+i reverse`. The panes view already states the rule this violates — the
other view outranks a way of moving around this one — so the branch ladder now follows it.
