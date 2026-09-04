# Usage

[日本語](../ja/usage.md)

Two pickers, opened by two actions, toggled with `Tab`. Each opens as a popup: a centred box
over the live session, framed by herdr in your accent colour and titled
`herdr-worktree-nav`, at the same proportions the session navigator uses.

Inside that frame they are drawn the way the navigator is — the same search line, breadcrumb
and key hint, the same connected tree glyphs, the same status glyphs. The keys the navigator
defines mean the same thing here.

While a picker is up, herdr routes every key into it, so its own keybindings are out of reach
until you close it. `Esc`, `q` and `Ctrl-C` all close it.

Where the picker opens *from* matters. It takes the repository and the pane you were on when
you pressed the key, which is how "split here" knows where "here" is and which repository the
branch list starts on.

## Panes

| Key | Does |
| --- | --- |
| `↑` `↓`, `k` `j`, `Ctrl-P` `Ctrl-N` | move |
| `←` `→`, `h` `l` | the head of the previous or next repository |
| `Enter` on a pane | go to it |
| `Enter` on a checkout with nothing running | open it |
| `n` | add a pane to the checkout under the cursor |
| `Shift-D` | delete the checkout under the cursor, after asking |
| `Tab` | branches, starting on the repository under the cursor |
| `/` | search |
| `b` `w` `i` `d` | narrow to blocked, working, idle, or done |
| `a` | clear the state filter |
| `r` | reload |
| `q`, `Esc`, `Ctrl-C` | close |

`←` and `→` — or `h` and `l`, as `↑`/`↓` are also `k`/`j` — land on the first thing worth
going to in a repository: its first pane, or its first checkout with nothing running. One
press is always exactly one repository, wherever in the current one you were: `←` from
halfway down leaves it rather than returning to its top. Both wrap, and the panes that are in
no repository count as a section like any other.

The arrows work while the search box has focus; the letters do not, because there they are
what you are typing.

Panes that are not inside a repository are always listed, in a section of their own at the
bottom. They are still panes, and a picker that hides some of them makes you wonder which.

The cursor stops only where there is somewhere to go: a pane, and a checkout with nothing
running in it. Repository headings and checkouts that already have panes are stepped over —
the panes listed directly under them are the answer, and stopping on the header first would
only make the walk longer. They stay on screen; the arrow keys just pass through them.

### Deleting a checkout

`Shift-D` offers to remove the checkout under the cursor — or, on a pane, the checkout that
pane is in. The question comes as a box over the list rather than as a line in the search
field, because this is the one thing the picker does that cannot be undone by doing it again.
`y` is the only key that answers it; anything else is a no.

```
   └── · fix/crash  no pane    ~/.herdr/worktrees/app/fix-crash

        ┌──────────────────────────────────────┐
        │ Delete this checkout?                │
        │                                      │
        │   fix/crash                          │
        │   ~/.herdr/worktrees/app/fix-crash   │
        │                                      │
        │   y delete     any other key cancels │
        └──────────────────────────────────────┘
```

The path is there because that, rather than the branch, is what is about to go. A long one
loses its middle; the breadcrumb under the list still carries the whole of it. In a pane too
short for the box the blank rows go first and the detail second, and the key hint says the
same thing either way.

It runs `git worktree remove <path>` and nothing else. The branch stays — a checkout is
rebuilt by making it again, and a branch that was never pushed is not. There is no `--force`,
so git refuses a checkout with uncommitted changes or untracked files in it and says so; that
refusal is the feature rather than an obstacle.

A finished worktree usually has panes in it — that is its ordinary end state — so `Shift-D`
closes them and then removes the checkout. They can be anywhere: this plugin moves a
worktree's pane wherever you asked for it, across tabs and spaces, and all of them stop. The
question names each one:

```
        ┌──────────────────────────────────────┐
        │ Delete this checkout?                │
        │                                      │
        │   feat/login                         │
        │   ~/.herdr/worktrees/app/feat-login  │
        │                                      │
        │   these panes close:                 │
        │   ● claude  working   w2:p1          │
        │   · shell             w2:p2          │
        │                                      │
        │   y delete     any other key cancels │
        └──────────────────────────────────────┘
```

Still one key. git protects uncommitted work and does not protect what a working agent has in
flight, and that list is the only safety net there is for it — which is why the panes outlast
the path when the box has to shrink.

The cursor does not stop on a checkout that has panes, because the panes listed under it are
the answer to where to go. Ask from the pane instead: `Shift-D` there is about the checkout it
is in, and the box names it before you answer.

Two things are refused before the question is even asked: the repository's own checkout,
because git cannot remove a main working tree and it is not a worktree anyway; and a checkout
with panes in it that is holding uncommitted work, or that git would not read at all. That
second refusal exists only where there are panes at stake: on an empty checkout git can answer
for itself, but here the panes would already be closed by the time it did.

`y` comes straight back. The removal runs somewhere else — git has to walk a whole working
tree before it can delete it, which is seconds on a repository of any size — and the row
says so while it does:

```
   └── · fix/crash  deleting ⠻    ~/.herdr/worktrees/app/fix-crash
```

The cursor steps over that row from then on: there is nothing left to do to it, and the next
thing to tidy up is usually the row below. Keep going, delete another, `Tab` to the branches
view, or close the picker — none of that waits for it, and closing costs nothing.

However it ends, herdr says so:

| | |
| --- | --- |
| removed | `removed fix/crash`, with the path underneath and no sound |
| refused | `could not remove fix/crash`, with git's own words underneath, and a sound |

A refusal that came after panes were closed says so — `… — its 2 panes were closed first` —
because that is the one failure that is not "nothing happened".

That notification is the report, because it is the one that still arrives when the picker has
gone. If yours are turned off you will not see it, and nothing is lost: a removal that was
refused leaves the checkout exactly where it was, so the row is there next time and
`Shift-D` on it will give you git's reason again. With the picker still up, a refusal is on
the prompt line as well.

`b`/`w`/`i`/`d` replace the search box with a state chip. Pressing the same one again clears
it, so a filter is never a one-way door.

While searching, letters are text rather than commands. `Enter` keeps the filter and returns
to the keys above, `Esc` abandons it, and `Ctrl-U` empties it without leaving search.

### Reading a row

```
 ◆ ● ShoMasegi/herdr-worktree-nav (2)
   └── ● main  ↑2↓1                    ~/Workspace/herdr-worktree-nav
 ◆    ├── ● claude                     w7:p2
      └── · shell                      w7:p3
   └── · fix/crash  ✱  gone  no pane   ~/.herdr/worktrees/herdr-worktree-nav/fix-crash
                                       ↑ four columns past the longest label
```

Left to right: a gutter, the tree, a status glyph, the label, and a meta column.

- **The gutter** carries `◆` on the pane the session is currently focused on, and on the
  repository holding it.
- **The tree** connects what is under a repository with `├──`/`└──`, so a deep worktree
  still reads as belonging somewhere. The repository itself gets no glyph: it is a heading,
  with nothing to expand.
- **The label** is `owner/repo (n)` for a repository, where `n` counts its open panes; the
  branch for a worktree; and the agent's name, or `shell`, for a pane.
- **The meta column** says where the thing is: the checkout path for a worktree and the pane
  id for a pane. A repository shows nothing there — the main checkout directly beneath
  carries the same path. Paths under your home are shortened to `~`.
- The column starts four blanks past the longest label that has something to put there, not
  at the right edge, so a path stays beside its row on a wide pane rather than across the
  screen from it. It takes the rest of the line from there, so a path is usually shown in
  full; one that still will not fit loses its middle rather than either end, since the head
  says which tree the checkout is in and the tail says which checkout.
- A checkout with nothing running in it is marked `no pane` beside its name, because its
  meta column is taken by the path.

### What a checkout is in the middle of

Between the branch name and the `no pane` note, a checkout says what state it is in. Most
have none of this, which is why it rides beside the name rather than in a column of its own:
a column that is blank on most rows is a permanent gap between the name and the path.

| | |
| --- | --- |
| `✱` | the working tree has uncommitted changes or untracked files |
| `?` | git would not read this working tree, so nothing is claimed about it |
| `↑2↓1` | two commits its upstream does not have, one it does not have |
| `gone` | git cannot find the ref this branch tracks |

`✱` is the one that stops `Shift-D` finishing: git refuses to remove a checkout holding work
nobody has committed, and this is that refusal in advance — the picker will still let you
ask, and git's answer comes back as a notification. `gone` means git
cannot find the ref the branch tracks; usually that is a merged pull request whose head
GitHub deleted and an `f` fetch in the branches view then pruned, but an upstream you have
never fetched reads the same, because to git it is the same.

Ahead, behind and `gone` come out of the same `git for-each-ref` the picker already runs, so
they are there in the first frame. Whether a working tree is dirty is not: git has to walk
the whole tree to know, once per checkout, so those are asked in the background and each row
is filled in as its answer lands. Until one does, the row says nothing rather than guessing,
and the prompt line carries a spinner so a list that is still filling in does not read as one
that found nothing.

The room for a `✱` is kept from the first frame whether or not one turns up, so an answer
landing never moves the paths beside it. Three columns is the price of a list that does not
shift while you are reading it.

When git will not answer at all, the row says `?` rather than nothing:

```
   └── · fix/crash  ?  no pane        ~/.herdr/worktrees/app/fix-crash
```

An unread working tree is the absence of an answer, not the answer `clean`, and without the
marker such a row is indistinguishable from one that was answered for. Both shapes of failure
need it: a `safe.directory` refusal, or a `git` that is not on the path herdr launched the
plugin with, fails the same way for every checkout at once — while a worktree whose directory
has gone out from under git fails for exactly one, on a list where every other row is fine.
It takes the room already kept for `✱`, so nothing moves.

`r` asks again. It is the only thing that does: the answers are otherwise kept for as long as
the picker is open, `Tab` to the branches view and back included.

The breadcrumb under the list carries the whole path for the row under the cursor, which is
where an elided one can be read in full.

A blank line separates each repository, and a scrollbar appears on the right when the list is
longer than the pane.

Agent state: `●` working, `○` idle, `◆` blocked, `·` no agent — or `◐`, `○`, `×`, `·` if your
herdr is set to `status_indicators = "symbols"`.

### Filtering

The filter is fuzzy and cascades down the tree: matching a repository shows everything in it,
matching a worktree shows the panes running on it, and a pane that matches on its own brings
its headers along so you can see where it is.

Rows that did not match themselves but are kept for context — the repository above a result,
the panes under a matching branch — stay in the list and are drawn dimmed, so a result is
never shown without the structure that explains where it is.

Fuzzy matching is permissive — `harken` is a subsequence of a surprising amount of text — so
results are ordered by how well they matched rather than by tree order. The repository you
meant is at the top. (herdr's navigator keeps session order instead; its rows are ordered by
something you already know, and these are not.)

## Branches

Three steps: which repository, which branch, and where its pane goes.

### Choosing a repository

Every repository herdr has open, which is the same set the panes view groups by. The one you
summoned the picker from is marked ◆ and starts under the cursor, so carrying on where you
were costs one more `Enter` — and any other repository is one ↓ away, without going through the
panes view first.

```
 / search repositories                                        4 repositories
 ◆ ShoMasegi/herdr-worktree-nav     1 worktree, 2 panes    ~/Workspace/herdr-worktree-nav
   ShoMasegi/harbour-backend  3 worktrees, 5 panes   ~/Workspace/harbour/harbour-backend
   nightowl/harken            1 worktree, 1 pane     ~/Workspace/nightowl/harken
   nightowl/harken_android    1 worktree, 3 panes    ~/Workspace/nightowl/harken_android
```

The counts say how much of each repository is already open. The path tells two checkouts of
the same fork apart, and is searchable along with the name.

| Key | Does |
| --- | --- |
| `↑` `↓`, `k` `j`, `Ctrl-P` `Ctrl-N` | move |
| `Enter` | list this repository's branches |
| `/` | search |
| `Tab` | back to panes |
| `q`, `Esc`, `Ctrl-C` | close |

With only one repository open this step is skipped: a picker that asks you to choose between
one thing is asking nothing. `Esc` then closes the branch list rather than going back to it.

### Choosing a branch

The list is headed by the repository the branches belong to and where it is on disk — the
same line the repository step had under its cursor, moved to the top. With one repository
open there is no repository step, so that heading is the only place it is named. Nothing
below repeats it: every row belongs to the same repository, so the breadcrumb under the list
starts at the branch.

| Key | Does |
| --- | --- |
| `↑` `↓`, `k` `j`, `Ctrl-P` `Ctrl-N` | move |
| `Enter` | choose this branch |
| `/` | search |
| `i` | next order |
| `Shift-I` | reverse it |
| `n` | start a branch from this one |
| `f` | fetch this repository |
| `Tab` | back to panes |
| `Esc` | back to the repositories |
| `q`, `Ctrl-C` | close |

### Starting a branch from another

`n` on a branch asks what to call a new one cut from it, and then where its pane should go —
the same destination step every other branch goes through.

```
 + new branch from main: feat/login-v2█                           5 branches
 me/app · ~/src/app ─────────────────────────────────────────────────────────
   ● feat/login    running      #123 Add the login screen (draft)
   · main          local        latest work on main
```

The prompt names what is being cut from, because that is the whole difference between this
and the `+ create` offer, which starts from `HEAD`. The base is settled the moment `n` is
pressed: the list stays on screen so you can see the row it names, but the cursor no longer
moves — a base that changed while you typed would not be one you chose.

`Ctrl-U` clears the name, `Esc` goes back to the list. `Esc` from the destination comes back
here with the name still typed.

A name is refused at the prompt — empty, one git would reject, or one this repository already
has — rather than after a destination has been chosen and the work has started.

Starting from a branch that has only ever been on the remote fetches it first and cuts from
`origin/<branch>`, exactly as choosing that row on its own does.

### Searching either list

Both lists work the way the panes view does: letters are commands until `/` gives the search
field the keyboard. The `/` at the left is dim while the list has the keys and takes the
accent, with a block cursor, while the search field does. The `search …` hint goes with it:
it is advice about a field you are not in.

| Key | While searching |
| --- | --- |
| any letter | filter |
| `↑` `↓`, `Ctrl-P` `Ctrl-N` | move |
| `Enter` | choose what is under the cursor |
| `Backspace` | delete a character |
| `Ctrl-U` | empty the search |
| `Ctrl-O` `Ctrl-R` | order and reverse |
| `Ctrl-F` | fetch |
| `Esc` | abandon the search |

`Enter` picks rather than committing the filter, which is where this differs from the panes
view: what you do with a narrowed branch list is open the one thing left in it, and asking
for a second `Enter` to say so would be ceremony.

`Esc` abandons what was typed, as it does in the panes view. Nothing is lost by that, because
the `Ctrl-` forms of the commands — `Ctrl-O` and `Ctrl-R` for the order,
`Ctrl-F` to fetch — keep working while you type. Reordering a filtered list does not cost you the filter.

Each repository is read once. Going back, picking another, and returning does not re-run git.

`Ctrl-F` runs `git fetch origin --prune` on the repository and reads it again. That is what
fills in the date and the commit subject for branches you have never fetched — until then
`git ls-remote` knows only their names — and what removes the ones that no longer exist on
the remote. It deletes nothing but remote-tracking refs, which are a cache of the remote:
no local branch and no working tree is touched.

`⠙ fetching origin…` sits beside the prompt in the accent colour while it runs, and the list
stays usable throughout. A fetch that cannot reach the remote says so on that line and
changes nothing.

The remote is read in the background. The local answer is on screen immediately and
`⠙ reading the remote…` sits beside the prompt until `git ls-remote` returns; branches that
have never been fetched appear when it does. Offline, that line simply goes away and the
local list stands.

Both waits carry the same spinner, turning on a clock rather than on redraws: a picker that
is busy should never look like one that is stuck.

### Ordering

`i` walks the three orders and `Shift-I` turns the current one around — `Ctrl-O` and
`Ctrl-R` do the same while the search field has the keyboard, since `Ctrl-I` is Tab in a
terminal and this view spends Tab on the panes. Which one is in
force sits beside the count, and takes the accent colour once it is no longer the default,
because a list in an unusual order should say so.

| Order | Reads |
| --- | --- |
| `state ↓` | running, then checked out, then local, then remote-only — newest first within each. The default |
| `updated ↓` | most recently committed first |
| `name ↑` | a to z |

The arrow describes the values rather than the rows: ↓ is descending, so it means newest,
busiest, or z first. Changing the order puts the arrow back to that
order's own direction, since "oldest first" is not what asking for a date order meant;
`Shift-I` is how you say you meant it.

Changing the order puts the cursor on the first row. What a new order is for is seeing what
is now at the top, and carrying the cursor along with the branch it was on would leave it
wherever that row happened to land — the one place the answer is not. Reversing does the same,
for the same reason: it is asked for to see the other end.

Two things stay put whichever order is chosen. A branch with no date — one seen only through
`ls-remote` and never fetched — sinks to the bottom in both directions, because reversing an
order should not fill the top of the screen with the rows that have the least to say. And the
offer to create a branch stays last: it is an offer, not one of the repository's branches.

The order also survives typing. The fuzzy search decides what is in the list; the order
decides where it sits. Sorting the filtered list by match score instead would quietly
override an order you had just chosen, the moment you typed anything.

It is kept while the picker is open, across repositories, and forgotten when it closes: this
plugin writes nothing to disk.

### What the states mean

| Shown as | The branch is | `Enter` does |
| --- | --- | --- |
| `● running` | open in a pane right now | goes to that pane |
| `○ checked out` | a worktree with nothing running | opens that checkout where you choose |
| `· local` | a local branch, no worktree | cuts a worktree from it |
| `↓ remote` | on the remote, never fetched | fetches it, then cuts from `origin/<branch>` |
| `+ create` | nothing yet — you typed it | creates it from `HEAD`, then cuts |

A branch can also be `gone`, which is not a state of its own but a note beside one:
`checked out gone`, `local gone`. It means git cannot find the ref the branch tracks —
usually a merged pull request whose head GitHub deleted, noticed by a pruning fetch. The
branch and its checkout are still here; what they were tracking is not.

A branch you have simply never pushed is not `gone`. It has nothing to track, which is a
different thing from tracking something that has gone, and the picker will not confuse the
two.

`running` skips the destination step. You already have that work open; being asked where to
put a second copy of it would be the wrong question.

`remote` is fetched into `refs/remotes/origin/<branch>` and the worktree is based on that
ref. Basing on `HEAD` instead would hand you an empty branch that merely shares a name with
the one on GitHub.

The offer to create sits last and appears whenever what you typed is a valid branch name that
does not already exist — including when something else fuzzy-matched. Typing `feat/login-v2`
while `feat/login` exists must still offer to create it.

### Choosing a destination

| Key | Does |
| --- | --- |
| `↑` `↓`, `k` `j` | move |
| `Enter` | open the pane there |
| `Esc`, `Backspace` | back to the branch list |

```
here            split right
                split down
existing tab    w1  app / logs
                w5  harken / android
existing space  w1  app → new tab
new space       on its own
```

- **here** splits the pane you summoned the picker from.
- **existing tab** splits that tab at whichever pane herdr thinks best. The tab you came from
  is not listed — "here" already covers it.
- **existing space** adds a tab to that space.
- **new space** leaves it in the workspace herdr made for it, which is what herdr's own
  `new worktree` binding does.

Beside the list is what the tab will look like once the pane lands in it, for whichever row
the cursor is on: the tab's real layout, with the arriving pane drawn in where it will
actually go. The prediction is exact rather than a sketch — a destination that names no pane
splits whichever pane that tab has focused, and the split is even, which is what herdr does.

A tab that is **zoomed** shows a warning instead of a diagram. herdr answers a request to
move a pane into a zoomed tab with success and then does not move it, so the picker stops
before asking rather than appearing to work. Unzoom the tab and it becomes available.

`split right` starts selected, so `Enter` `Enter` puts the branch beside what you were doing.

### While it works

Opening a branch can mean a fetch across the network and a checkout of a whole working tree.
The picker stays where it is and says which step it is on:

```
 ⠸ fetching origin/feat/login…
 ─────────────────────────────────────────────────────────────────────
 here            split right       w1  app / agents
                 split down        ┌──────────┬──────────┐
 existing tab    w1  app / logs    │ ● claude │ + feat/l…│
 …
 ctrl+c stop
```

The list and the preview stay put — the highlighted row is the destination being acted on,
and the diagram beside it is the tab being built. The steps are `fetching origin/<branch>`,
`creating the worktree for <branch>` or `opening the checkout for <branch>`, and then moving
the pane where you asked.

`Ctrl-C` stops it, but only while the key hint says `ctrl+c stop` — that is, up to and
including the fetch. A fetch writes nothing but `refs/remotes` and can be walked away from.
Once herdr has been asked for a worktree, leaving would strand the workspace it made, so
there is no key for it and the hint changes to `working…`.

If a step fails, the picker holds the screen and shows what git or herdr said, on the step
that said it:

```
 × fetching origin/feat/login: `git fetch …` failed: fatal: Could not read from remote…
 …
 ↵ close  esc close
```

Nothing is left half-done: the failing step is the one that stopped, and the steps before it
either did not touch anything (the fetch) or completed. The same message also goes to
`herdr plugin log list`, in full.

## Diagnostics

```sh
herdr-worktree-nav dump
```

Prints the tree the picker would draw, as plain text. Useful for separating "herdr or git
told us something odd" from "the picker drew it wrong". Run it from a pane inside a herdr
session — it needs `HERDR_SOCKET_PATH`.
