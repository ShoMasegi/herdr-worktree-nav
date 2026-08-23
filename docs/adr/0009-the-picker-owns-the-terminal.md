# 9. The picker owns the terminal, and the views borrow it

Status: accepted

## Context

`Tab` is documented as toggling between two views of one UI. It was implemented as two
programs taking turns. Each of `panes::run` and `branches::run` called `ratatui::try_init()`
on the way in and `ratatui::try_restore()` on the way out, so every press of `Tab` did this:

1. `LeaveAlternateScreen` — the pane falls back to its primary screen, which in a plugin pane
   is empty, so herdr's popup frames nothing at all.
2. `thread::scope` joins the listing threads the branches view spawned. One thread per
   repository, running `git ls-remote` and then `gh pr list` — 464 ms and 428 ms measured
   here, so about 0.9 s if they had only just started.
3. `collect::collect_tree` runs again for the incoming view — 61 ms.
4. `EnterAlternateScreen`, and only now is there something to look at.

One press was a flicker. Held down, the presses queue in the tty and the gap becomes
continuous: eight rapid presses took 2.48 s, nearly all of it with the popup empty. It was
reported as "nothing is displayed any more", which is exactly what it looks like — the
switching itself was correct the whole time.

This is the same shape as the bug [ADR 0007](./0007-stay-up-while-working.md) fixed for
opening a branch: a restored terminal inside a popup herdr is still holding open is not a
neutral state, it is a blank one.

## Decision

`run_picker` calls `try_init` once, before either view runs, and `try_restore` once, after
the last one returns. The views take `&mut DefaultTerminal` and never touch the terminal's
lifetime. `Tab` now changes what is drawn and nothing else.

The switching loop is a separate function, `views`, whose result is captured before the
terminal is put back. That is what makes the restore unconditional: every `?` inside the loop
lands on it. When both the loop and the restore fail, the loop's error is the one raised —
a picker that fell over says more than a terminal that would not go back.

What each repository's remote answered is hoisted out of `branches::run` alongside it, into a
`domain::listing::Cache` that `views` owns. Coming back to the branches view then costs a
`git for-each-ref` (8 ms) rather than another network round trip, and — because the listing
threads are what step 2 above waits for — there is nothing left to join on the way out.
Whatever is still in the channel once `thread::scope` has joined is drained into that cache,
so an answer that arrived after the last frame is not thrown away.

## Consequences

**A herdr call now happens with the alternate screen still up.** `perform` runs before
`run_picker` restores, which reverses the order `panes::run` used to promise in its doc
comment. The promise was never about ordering, though — it was that a failure has to be
readable rather than land on a screen the picker has scribbled over. It still is: the error
travels up through `views`, the terminal goes back, and `main` prints it after.

**`ratatui`'s panic hook is installed once instead of once per switch.** `try_init` calls
`set_panic_hook`, which takes the current hook and wraps it. Nine switches used to leave nine
nested hooks, each restoring the terminal again on the way down. Nobody noticed, because
restoring an already-restored terminal is harmless — but it was growing.

**Local refs are re-read on every visit and the remote is not.** They are the half that
changes while the picker is up and the half that is nearly free; the cache holds only what
cost a round trip. A `git fetch` drops the repository's entry outright rather than patching
it, because `--prune` deletes refs and anything merged in would put them straight back.

**Leaving the picker still exits the process rather than the loop** where the branches view
already did so. Nothing here changes that; the reason is unchanged and is written down in
ADR 0007.

**The two views are still two functions.** Merging them into one state machine would follow
"they are one UI" further, but the branches view's worker thread, its tick, and its progress
display do not have counterparts in the panes view, and none of that was what made `Tab`
blank the screen.

## Measured

Eight rapid presses of `Tab`, driven through a pty at 120x40 against a live session:

| | alternate screen entered | elapsed |
| --- | --- | --- |
| before | 9 times | 2.48 s |
| after | 1 time | 1.19 s |

The remaining growth is `collect_tree` per switch, about 50 ms: seventeen presses take
1.50 s, not nine times as long as two.
