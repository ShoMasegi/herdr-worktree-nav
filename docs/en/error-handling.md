# Error handling

What a failure is allowed to turn into, which failures a change has to answer for, and where
a failure ends up on screen.

This page exists because the same defect kept being found one instance at a time, in review,
after the design was settled. The rules below are mostly not new — they were already being
applied, in doc comments and in [the records](../adr/) — and writing them down is meant to
stop each pull request rediscovering them.

## What an error is here

This is a picker. Almost nothing it does is a computation that could have gone wrong on its
own: nearly every failure is something the outside world said. git refused, herdr is not
answering, `gh` is not installed, the config file has a bad line.

So a failure here is nearly always **a sentence somebody has to read**, not a value the code
branches on. That is why there is one error type — `anyhow::Error` — and no enum of our own.
A richer type would be earning its keep if callers chose different behaviour per variant, and
they do not; they choose between *saying it* and *not saying it*, which is what the rest of
this page is about.

Two consequences:

- **git's and herdr's own words are the message.** We do not paraphrase them. The adapter
  adds which command was run; everything above passes the words up. `one_line` flattens them
  for a screen that has one line, and that is the only edit they get.
- **The words have to survive the trip.** A failure that is caught and re-worded into our own
  prose loses the half a person troubleshooting needs — which is why the locale git speaks is
  pinned in exactly one place, and why `src/` may start exactly one git. Both are held by
  `scripts/check-invariants.sh`.

## The rule that matters

> **A failure may never become a value that a legitimate answer could also have produced.**

This is the whole of it. Every other rule on this page is a consequence.

`Ok(false)` from a `git status` that could not look is indistinguishable from a clean working
tree, and a sweep deletes on the strength of it. `Ok(None)` from a `git remote get-url` that
hit a bad `.git/config` is indistinguishable from a repository with no `origin`, and the user
is told to look at their remotes. `None` from a `git` that could not start is
indistinguishable from a pane outside any repository, and the whole session draws as
ungrouped with nothing to say why.

The type system already carries the distinction where it has been needed, and the shape to
copy is there:

- `WorkingTree` is `Clean | Dirty | Unreadable`, not a `bool`. "Not dirty" is the wrong fact
  to delete a checkout on, because it also means *nobody has asked yet* and *git would not
  say*.
- `Refs` is `Read | Unreadable(String)`, carrying the words, because a checkout with no
  markers and a checkout whose markers could not be read look the same on a row.

**When a failure has to be flattened anyway, say what is being swallowed.** Some flattening
is right: a `gh` that is missing, unauthenticated or offline degrades to an empty list on
purpose, and [ADR 0003](../adr/0003-git-first-gh-optional.md) is why. What separates that
from a defect is not the operator used — it is whether the comment above it names every
failure it takes in and says why none of them needs telling apart from the normal answer.

Compare. This one is right, and says so:

```rust
// A checkout git could not answer for gets no marker — no marker beats the wrong marker —
// but it is not recorded as clean, because that is a claim and this is the absence of one.
let dirty = git.is_dirty(checkout_path.as_str()).ok();
```

This one asserts an equivalence that does not hold, and is issue #33:

```rust
// A path that is not in a repository, and a git that failed, are the same thing here:
// the pane is simply not grouped.
let identity = git.identify(cwd).ok().flatten()?;
```

So: a site that turns a port's `Result` into a plain value carries a `// swallows:` line
naming what can arrive there and why it need not be told apart. A site with no such line is
read as an oversight, not as a decision.

## Four classes, and what each one gets

The useful question is not how bad a failure is. It is **what the person or the code
concludes from the value, if the value is wrong.**

### C1 — a wrong claim

The failure arrives as a value saying something is safe or settled, and something acts on it:
a removal goes through, a marker is drawn, a sentence names the wrong cause.

**Always fixed, and it blocks the change that is in front of it.** This is the only class
that is not negotiable, because it is the only one that costs the user something they cannot
get back.

### C2 — a silent degradation

The failure arrives as "nothing to report". Nothing false is shown and nothing is destroyed,
but the picker is quietly doing less than it claims, and no part of the screen says so.

**Fixed by carrying the words and putting them on the prompt line** — the plumbing for that
is the point of the notice mechanism, and adding one should be a line, not a design.

### C3 — a reporting defect

The failure is detected and reported. The wording, the placement, the language or the moment
it disappears is wrong.

**Does not block a change.** Filed, and fixed in a batch with its siblings.

### C4 — a limit we accept

The honest answer is that we do not look. `git status --porcelain` cannot see work under an
ignored directory, and no amount of reading its output will change that; seeing it means
asking a different question.

**Written down as a limit and closed.** The distinction that matters to a later reader is
between *nobody noticed* and *we decided not to*, and only the second one can be recorded.

## Triage in review

A reviewer finding an edge case is the normal case, not a failure of the change. What costs
rounds is re-deciding, every time, whether this one belongs to this pull request.

> **A finding blocks the pull request in front of it when either:**
>
> **(a)** it is C1, or
> **(b)** this change is what made it reachable, or made it load-bearing for the first time.
>
> **Everything else is filed.** Filing is not "we will not fix this" — it is "this is not the
> change that has to."

(b) is the one that does the work. A defect can be years old and still be this change's
business: [issue #56](https://github.com/ShoMasegi/herdr-worktree-nav/issues/56) is a
`worktree.list` failure that had been dropped in silence since long before the sweep existed,
and it became blocking when a sweep's `Enter` made a second listing load-bearing.

The reviewer's job is therefore to **classify**, not to argue. A class and a number are a
complete review comment.

When a finding is filed rather than fixed, the issue says where it was found and whether it
is pre-existing, so the next reader can tell a deferral from an oversight.

## Where a failure goes on screen

Failures reach the user in three places, and the difference between them is lifetime.

**A condition** is true until something measures it again: a repository whose refs git would
not read, a list that is behind. It is **derived from state every frame**, never pushed and
never stored. That is what makes it retire correctly — a frame where the condition no longer
holds simply does not produce the sentence, so nothing has to remember to take it back.

**An event** is true at a moment: a removal was refused, a key did not apply here. It is
**pushed**, and it retires on the next act that could have been a response to it.

Getting these two the wrong way round is what
[#64](https://github.com/ShoMasegi/herdr-worktree-nav/issues/64) and
[#35](https://github.com/ShoMasegi/herdr-worktree-nav/issues/35) are: a condition pushed as an
event outlives the thing it described, and a condition treated as a placeholder loses the line
to a filter chip that had sixty columns free beside it.

**A blocking failure** is the third, and it is different in kind: a step the user asked for
failed, and the screen is held so the reason can be read. Without it the popup would vanish
the instant the process ended, which is indistinguishable from having worked.

Two standing rules for all three:

- **No width may print a blank where a failure would be.** A line too narrow for the sentence
  shows a count, never nothing. The failure mode being avoided is a picker that looks healthy
  because the terminal was small.
- **`dump` is the unabridged copy.** One line can never hold what git said in full, so the
  subcommand that a bug report asks for prints every failure whole, per repository. The
  screen summarises; `dump` is where the words are.

## Panics

Shipped code does not panic. The release profile is `panic = "abort"`, so a panic takes the
picker down with the process rather than surfacing anywhere a user could report from.

`unwrap` and `expect` belong in tests and in test fakes. In `src/` outside `#[cfg(test)]`
they want a reason in a comment, and the reason has to be about why the value cannot be
absent — not about it being unlikely.

A thread that panicked is the one case where a `join` failure is possible, and in the shipped
binary it is unreachable for the reason above. `app::collect` writes that down rather than
leaving the arm looking load-bearing.
