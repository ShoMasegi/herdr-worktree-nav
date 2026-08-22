# 6. Start the branches view on a repository, and let the user order the branches

Status: accepted

## Context

The branches view listed the branches of exactly one repository: the one the picker was
summoned from, or the one under the cursor when `Tab` was pressed in the panes view. To get
at another repository's branches you had to first be somewhere inside it, or go to the panes
view, find a pane in it, and press `Tab` there.

That is backwards for the thing the view is for. Choosing a branch to start work on is not
something you only ever do in the repository you are already looking at — often it is the
opposite, because the reason you are opening a picker is that the work you want is somewhere
you are not.

The list also had one fixed order: state rank, then most recent, then name. Good defaults,
but a fixed order is a guess about which question is being asked, and there is more than one
question.

## Decision

**A repository step in front of the branch list**, listing every repository herdr has open —
the same set the panes view groups by, taken from the snapshot the picker already fetches, so
it costs no additional I/O. The repository the picker was summoned from is marked and starts
under the cursor.

It is skipped when only one repository is open. A picker that asks you to choose between one
thing is asking nothing.

herdr has no "repositories I know about" API, so this is the only set that can be listed
without either scanning the disk or asking GitHub. A repository with no pane open anywhere is
not in it. That is a real limit, and the right one for now: this plugin's whole model is the
session herdr is holding, and a repository outside it has no pane to reach and no worktree
record to consult.

**`Esc` goes back one step** everywhere, rather than closing from the branch list. The
destination step already behaved this way; three steps under one rule beats two rules.

**Three orders, chosen with `Ctrl-O` and reversed with `Ctrl-R`.** The old fixed order becomes
`state`, which stays the default, so an untouched picker looks exactly as it did.

## Consequences

Four things follow from this that a future reader might otherwise undo.

**The keys are `Ctrl-` keys.** The branch list was a search box with no mode to enter, so `o`
and `r` were branch names being typed, not commands. Any ordering control had to be a chord.

> Superseded in part: the branch list gained the panes view's `/` mode, so `o` and `r` are
> also plain letters now. The chords stayed — they are what lets an order be changed without
> abandoning what has been typed, which is the whole reason `Esc` can afford to discard it.

**The chosen order beats the fuzzy score.** Filtering used to sort by match score. It now
sorts by the chosen order, and the score only decides membership. Score sorting is genuinely
useful — but it would silently override an order the user had just chosen, the moment they
typed anything, and a control that stops working when you use the other control is worse than
one that never existed. After filtering the list is short; best-match-first buys little there.

**A branch with no date sinks to the bottom in both directions.** A never-fetched remote
branch has no committer date at all. Treating `None` as the smallest value is the obvious
implementation and it is wrong at the moment it matters: reversing "by date" would fill the
top of the screen with the rows that have the least to say.

**Changing the order resets the direction to that order's own.** Nobody asking for a date
order means "oldest first". Carrying a reversal across a change of key would make `Ctrl-O` a
surprise.

The order is held while the picker is open, across repositories, and forgotten when it
closes. Persisting it would mean writing a file, and this plugin currently writes nothing —
a property worth more than saving a keystroke.

Each repository's branches are read once and cached for as long as the picker is open, so
walking back and forth between repositories does not re-run git. A background answer carries
the repository it is about, because the user may well have moved on before it arrives.
