# 17. A module is split by what it is responsible for, and it composes no words it cannot see

Status: accepted

## Context

Two files passed two thousand lines while the picker was being built out, and a third passed
three and a half.

Nothing in them was wrong. `src/ui/render.rs` drew both pickers, every prompt, every
question box and the preview diagram, and every one of those was drawn correctly.
`src/ui/state.rs` held the panes picker's list, its sweep and its keymap.
`src/domain/rows.rs` worked out what the list is and, in the same breath, wrote what every
row says. What they had stopped being was modules with an answer to "what is this for". A
reader looking for what `q` does had to find which of six modes they were in first, and a
reader changing the word `shell` had to know that a layer with no access to a terminal was
the one that chose it.

The second half of that is the sharper problem. The same tab was named in the row that
offers it, in the caption over the preview of it, and in the step that lands a pane in it —
three strings built independently in two layers, free to disagree, with nothing that would
notice. `domain` is the layer that cannot see the screen, has no width to fit into and no
theme to read; it is the worst-placed part of the tree to decide how anything reads, and it
was deciding.

## Decision

**A module is split along what it is responsible for, not down the middle.** When a file
outgrows a reader, the cut follows the question it answers: `ui::panes` is the list, the
sweep and the keymap; `ui::render::branches` is the step on screen, the two lists and the
destination; `domain::rows` is the row, the shape of the list and the cursor's path through
it. A file split at its midpoint is two files neither of which is about anything.

**A directory module keeps the path.** `foo.rs` becomes `foo/mod.rs`, so nothing outside has
to be told. The siblings are private to it and widen one method at a time, which is what
keeps the split honest: a module that needed half its neighbour's insides made `pub(super)`
was cut in the wrong place.

**`domain` composes no user-facing text.** It answers what is true with a value — a
`TabName`, a `Landing`, a `Condition`, a `Refusal`, a `Row` carrying a name and a count —
and `ui::words` is the single place that turns any of those into a sentence. So the row, the
caption and the step read the same because they are the same call, and a change to the
wording is a change to one file.

**`app::words` is the exception, and names itself one.** `herdr-worktree-nav remove` runs
detached, with no terminal and no `ui` — [ADR 14](./0014-removing-outlives-the-picker.md) —
and `--dump` writes a page rather than a screen. Their words live beside the code that emits
them. What matters is that `domain` has no such words at all, so the rule has one answer
rather than two.

**A test fixture lives beside the thing it is a fixture for.** `domain::rows::fixtures` is
the tree the shape of the list, the cursor's path and the words drawn on it are each asked
about, and it is one tree because three copies of it would drift.

**Check 8 in `scripts/check-invariants.sh` caps a module at 800 lines of code.** Code, not
file: the count stops at the first `#[cfg(test)]`, and a file that is only test support is
not counted. Trimming tests to fit a number is the opposite of what the cap is for.

## Consequences

**The cap does not say where to cut.** It says the cut is overdue. A module pushed over it
by real growth is a module that has taken on a second responsibility, and the fix is to name
the second one, not to move a hundred lines somewhere convenient.

**Wording is testable in one place, and tested there.** The tests that assert what something
reads as live in `ui::words`; the tests in `domain` assert values. A domain test that
compares a string against English is a test in the wrong module — and, since `domain` may
not import `ui`, one the invariants already refuse.

**A `Row` is less convenient and more honest.** `name` is an `Option`, because a pane herdr
tracks no agent in genuinely has no name; `panes` is a count rather than a heading. Every
caller now says what it wants that to read as, which is the point.

**Snapshots follow their module.** `insta` finds a snapshot next to the source file that
asserts it, so splitting a file that carries snapshots moves them. The names are keyed on
the module path, so a split that keeps the path keeps every name — and a rename of the
module is a rename of forty files.
