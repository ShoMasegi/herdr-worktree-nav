# Error handling

What can go wrong while the plugin is being used, what each kind of failure is allowed to
turn into, and where it ends up on screen.

## Where failures come from

The picker computes almost nothing that can fail on its own. Nearly everything that goes
wrong is something outside it said, and there are only four outside things:

- **herdr**, over its socket. The session itself — panes, tabs, workspaces, worktrees — and
  every act on it.
- **git**, as a child process. Repository identity, refs, working trees, fetches, removals.
- **`gh`**, as a child process. Which pull requests have landed, and nothing else.
- **the plugin's configuration file**.

The terminal is a fifth, and a different sort: it is never the thing that fails, but it
decides how much of a failure a user gets to read. It has its own rules at the end.

So a failure here is nearly always **a sentence somebody has to read**, not a value the code
branches on. That is why there is one error type — `anyhow::Error` — and no enum of our own.
A richer type would earn its keep if callers chose different behaviour per variant, and they
do not; they choose between *saying it* and *not saying it*, which is what the rest of this
page is about.

Two consequences:

- **git's and herdr's own words are the message.** We do not paraphrase them. The adapter
  adds which command was run; everything above passes the words up. `one_line` flattens them
  for a screen that has one line, and that is the only edit they get.
- **The words have to survive the trip.** A failure that is caught and re-worded into our own
  prose loses the half a person troubleshooting needs — which is why the locale git speaks is
  pinned in exactly one place, and why `src/` may start exactly one git. Both are held by
  `scripts/check-invariants.sh`.

## Five kinds of failure

The kinds below are not sorted by which tool failed. The same refusal from the same command
belongs to a different kind depending on what is left of the user's world afterwards, and
that is the only question worth asking: **what is the user holding once this has happened?**

### 1. The plugin cannot go on

Nothing worth drawing is left: the socket path is not in the environment, the first snapshot
never arrived, an act the picker cannot proceed without has failed under it. There is no
screen to say it on, because there is no screen.

**What it gets:** the failure travels up to `main`, which prints it on stderr and exits with
`ExitCode::FAILURE`. herdr keeps that for `herdr plugin log list`, which is where the
troubleshooting page sends people.

**The rule:** this route is for a failure that leaves nothing to draw. A picker that could
still show a list and one fewer marker never takes it — that is kind 3.

### 2. Something the user asked for did not happen

A key was pressed, an act was attempted, and it failed or was refused. The user is owed an
account of it, because they are the one who asked and nothing else on screen will say so.
The picker itself is intact.

**What it gets:** one sentence, in whichever of the three places the user is still looking
at.

- **The picker is up.** The sentence is pushed onto the prompt line, and the next key takes
  it back — an account of a moment retires on the next act that could have answered it.
- **The picker has become a progress display.** Opening a branch is seconds of fetching and
  checking out, and the popup dies with the process, so a failure there holds the screen:
  `Activity::Failed` keeps the `Stage` it failed on and the words, and only a key that means
  *I have read it* closes the picker. Without the hold, a failure and a success look
  identical — the popup vanishes either way.
- **The act outlives the picker.** A removal runs in a session of its own
  ([ADR 0014](../adr/0014-removing-outlives-the-picker.md)), so there is no window left to
  write in and the answer comes back as a herdr notification. A refusal carries git's own
  words, because they say what would have been lost, and it is the one notification that
  makes a sound: it has to reach somebody who has stopped looking.

Much of this kind is not a defect at all. `RemovalOutcome::Refused` is usually git doing its
job — a checkout with uncommitted work is exactly what `git worktree remove` protects — and
`RemovalOutcome::BranchKept` is a branch that is not merged anywhere. They are handled the
same way as a real failure, because from the prompt line the difference is one the user makes,
not one we can make for them.

### 3. The picker is running, but knows less than it claims

Nothing the user did failed. What failed is a reading behind the rows: the tree could not be
read again, a repository's refs would not enumerate, `gh` refused during a sweep. The list is
still on screen and still looks authoritative, and it is out of date or incomplete.

**What it gets:** a **condition** — a sentence that stays true until something measures it
again, shown beside the prompt for as long as it holds.

The lifetime is the whole point, and it is why a condition is not a message. A condition is
**derived from state every frame** by `notice::conditions`; it is never pushed and never
stored as a sentence. A frame in which the thing is no longer wrong simply does not produce
the sentence, so nothing has to remember to take it back. Conditions retire when a reading
works, not when a key is pressed — pressing a key does not make a stale list fresh.

A condition is also never the *only* sign: the rows it is about carry no marker they have not
earned, which is kind 4.

### 4. An outside tool answered, and the answer cannot be trusted

git ran and came back, but it could not look: a working tree it would not read, refs it would
not enumerate. There *is* a value to store, and every ordinary value in range is a lie.

**What it gets:** a state of its own, so that "we do not know" cannot be read as an answer.
`WorkingTree` is `Clean | Dirty | Unreadable`, not a `bool`. `Refs` is
`Read | Unreadable(String)`, carrying the words. The row then draws no marker — no marker
beats the wrong marker — and a condition (kind 3) says why the markers are missing.

This is the kind that costs the most when it is got wrong, because nothing looks broken. It
is the subject of the rule in the next section.

### 5. A degradation we chose

An optional dependency is missing, unauthenticated, offline or slow, and the picker carries
on with a stated default rather than failing anything:

- **`gh`** degrades to an empty list of landed pull requests, for the reason
  [ADR 0003](../adr/0003-git-first-gh-optional.md) gives: that layer is decoration and must
  never fail the picker. It is also given a budget (`GH_BUDGET`), because a `gh` that never
  answers would otherwise hold a sweep open indefinitely, and a sweep that never gets its
  answer is a sweep that degraded without saying so.
- **The configuration file** degrades to the defaults when it cannot be read or parsed, and
  names itself on the prompt line as it does so. A typo in one key must not take the picker
  down.

**What makes this a kind rather than an excuse:** the default is written down here or in a
record, *and* the picker says out loud that it took it. A degradation nobody is told about is
kind 3 with nobody watching, and a degradation nobody wrote down is an accident that has not
been noticed yet.

## The rule the edge cases turn on

> **A failure may never become a value that a legitimate answer could also have produced.**

Every edge case worth arguing about is an instance of this, and the arguing stops once the
rule is applied instead.

`Ok(false)` from a `git status` that could not look is indistinguishable from a clean working
tree, and a sweep deletes on the strength of it. `Ok(None)` from a `git remote get-url` that
hit a bad `.git/config` is indistinguishable from a repository with no `origin`, and the user
is told to look at their remotes. `None` from a `git` that could not start is
indistinguishable from a pane outside any repository, and the whole session draws as
ungrouped with nothing to say why.

The three are the same defect in three places: a kind 4 failure was flattened into the shape
of an ordinary answer, and after that no amount of care downstream can get the distinction
back.

### When a failure is flattened anyway, say what is being swallowed

Some flattening is right — kind 5 is flattening on purpose. What separates it from a defect
is not the operator used; it is whether the comment above it names every failure that can
arrive there and says why none of them needs telling apart from the normal answer.

This one is right, and says so:

```rust
// A checkout git could not answer for gets no marker — no marker beats the wrong marker —
// but it is not recorded as clean, because that is a claim and this is the absence of one.
let dirty = git.is_dirty(checkout_path.as_str()).ok();
```

This one asserted an equivalence that does not hold, and was
[issue #33](https://github.com/ShoMasegi/herdr-worktree-nav/issues/33) — the shape a site
takes when the comment argues the failure away instead of naming it:

```rust
// A path that is not in a repository, and a git that failed, are the same thing here:
// the pane is simply not grouped.
let identity = git.identify(cwd).ok().flatten()?;
```

`app::collect::identify_one` answers `Result<Option<PanePlacement>, String>` now, so the two
are told apart and the prompt line says which.

So: a site that turns a port's `Result` into a plain value carries a `// swallows:` line
naming what can arrive there and why it need not be told apart. A site with no such line is
read as an oversight, not as a decision.

### Which findings a change has to answer for

An edge case found in review is the normal case, not a failure of the change. What costs
rounds is re-deciding, every time, whether this one belongs to this pull request.

> **A finding blocks the pull request in front of it when either:**
>
> **(a)** the failure becomes a value something acts on — a removal goes through, a marker is
> drawn, a sentence names the wrong cause; or
> **(b)** this change is what made it reachable, or made it load-bearing for the first time.
>
> **Everything else is filed.** Filing is not "we will not fix this" — it is "this is not the
> change that has to."

(a) is the only class that is never negotiable, because it is the only one that costs the
user something they cannot get back. (b) is the one that does the work: a defect can be years
old and still be this change's business.
[Issue #56](https://github.com/ShoMasegi/herdr-worktree-nav/issues/56) is a `worktree.list`
failure that had been dropped in silence since long before the sweep existed, and it became
blocking when a sweep's `Enter` made a second listing load-bearing.

What gets filed rather than fixed is one of three things, and the issue says which: a **silent
degradation**, where nothing false is shown but no part of the screen admits the picker is
doing less than it claims; a **reporting defect**, where the failure is caught and said, and
the wording, placement, language or moment it disappears is wrong; or a **limit we accept**,
where the honest answer is that we do not look — `git status --porcelain` cannot see work
under an ignored directory, and seeing it means asking a different question
([issue #68](https://github.com/ShoMasegi/herdr-worktree-nav/issues/68)). Only the third
one is closed. The distinction a later reader needs is between *nobody noticed* and *we
decided not to*, and only the second can be recorded.

## Asking again

A failure that fixes itself and a failure that never will are treated differently, and the
difference is whose it is:

- **`r` re-reads everything**, and also forgets what `gh` said, since a pull request can land
  while the picker is up.
- **Entering a sweep re-reads the tree and asks `gh` again where it refused last time**
  (`Settled::forget_failures`), because a `gh` refusal is usually a network or a token and
  those do fix themselves between one `Shift-S` and the next. It keeps the answers `gh` gave,
  which only widen the sweep.
- **A failure nothing outside will fix is not worth asking again.** A `.git/config` that will
  not parse answers the same way every time.
  [Issue #69](https://github.com/ShoMasegi/herdr-worktree-nav/issues/69) is that distinction
  not yet being drawn.

Nothing retries on its own. A retry the user did not ask for spends a process on a failure
that is usually deterministic, and hides it while it does.

## Failures that come back on a thread

Three things run beside the picker rather than in it: the walk that asks whether each checkout
is dirty, the `gh` query behind a sweep, and a removal in its own process. All three can fail
after the frame that started them is gone, so none of them may return a failure by holding
the loop.

Each reports the same way it reports success — one answer per checkout, per repository, per
removal — and the failure is a value in that answer, of kind 4 or kind 5. A thread that
cannot say *this one failed* in the same channel it says *this one is clean* has nowhere to
put the failure but a log, and that is how it goes silent.

## What the screen owes a failure

- **No width may print a blank where a failure would be.** A line too narrow for the sentence
  shows a count, never nothing. The failure mode being avoided is a picker that looks healthy
  because the terminal was small. It is also why a condition and the prompt line do not
  compete for the same columns: [#35](https://github.com/ShoMasegi/herdr-worktree-nav/issues/35)
  was a condition losing the line to a filter chip that had room to spare beside it.
- **What a width cut has to be readable somewhere the user already is.** `!` opens a panel
  over the list holding every condition whole. A count is enough to say *something is
  wrong*; it is not enough to act on, and a person who has to quit the picker and run
  another command to read a sentence will not read it. The panel degrades the way the line
  does — what does not fit becomes a count of what is left — because it is only ever read,
  and a clipped sentence is not a different sentence the way a clipped question is a
  different question.
- **Do not push what you should derive.** A condition stored as a message outlives the thing
  it described; [#64](https://github.com/ShoMasegi/herdr-worktree-nav/issues/64) is exactly
  that, and the fix is the shape in kind 3 rather than a place to take it back from.
- **`dump` is the unabridged copy.** One line can never hold what git said in full, so the
  subcommand a bug report asks for prints every failure whole, per repository. The screen
  summarises; `dump` is where the words are.

## Adding a call to the outside

A new port method, or a new caller of one, answers five questions before it is reviewable:

1. Which of the five kinds is it when it fails? If the answer is "it cannot fail", it is
   going through a port, so it can.
2. What does the caller do with the failure — hold, push, derive, or store a state of its
   own?
3. If the `Result` is flattened, what is the `// swallows:` line?
4. Does anything downstream now have a value it cannot tell from an ordinary answer?
5. Does a failure need asking again, and does it fix itself if you do?

## Panics

Shipped code does not panic. The release profile is `panic = "abort"`, so a panic takes the
picker down with the process rather than surfacing anywhere a user could report from.

`unwrap` and `expect` belong in tests and in test fakes. In `src/` outside `#[cfg(test)]`
they want a reason in a comment, and the reason has to be about why the value cannot be
absent — not about it being unlikely.

A thread that panicked is the one case where a `join` failure is possible, and in the shipped
binary it is unreachable for the reason above. `app::collect` writes that down rather than
leaving the arm looking load-bearing.
